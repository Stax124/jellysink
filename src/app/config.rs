use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub(crate) config_dir: PathBuf,
}

impl Paths {
    pub fn from_override(config_dir: Option<PathBuf>) -> color_eyre::Result<Self> {
        let config_dir = match config_dir {
            Some(p) => p,
            None => {
                let dirs = directories::ProjectDirs::from("", APP_NAME, APP_NAME)
                    .ok_or_else(|| usage_err("could not resolve a config directory"))?;
                dirs.config_dir().to_path_buf()
            }
        };
        Ok(Self { config_dir })
    }

    /// Creates the config directory, mode 0700 — `mpv.sock` is created by mpv
    /// under its own umask and hands out the access token, so the directory
    /// around it is what keeps it private. Re-applied on every run.
    pub(crate) fn ensure(&self) -> color_eyre::Result<()> {
        fs::create_dir_all(&self.config_dir)
            .wrap_err_with(|| format!("creating {}", self.config_dir.display()))?;
        fs::set_permissions(&self.config_dir, fs::Permissions::from_mode(0o700))
            .wrap_err_with(|| format!("restricting {}", self.config_dir.display()))?;
        Ok(())
    }

    pub(crate) fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub(crate) fn cred_file(&self) -> PathBuf {
        self.config_dir.join("cred.json")
    }

    pub(crate) fn lock_file(&self) -> PathBuf {
        self.config_dir.join("instance.lock")
    }

    pub(crate) fn stop_socket(&self) -> PathBuf {
        self.config_dir.join("stop.sock")
    }

    pub(crate) fn mpv_socket(&self) -> PathBuf {
        self.config_dir.join("mpv.sock")
    }

    pub(crate) fn mpv_args_file(&self) -> PathBuf {
        self.config_dir.join("mpv_args.conf")
    }
}

/// Every user-facing configuration key. Matching on it is exhaustive, so a new
/// key is a compile error until every place that handles keys handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    MpvPath,
    /// Lives in `mpv_args.conf`, re-read on every mpv spawn.
    MpvArgs,
    LogLevel,
    Autoplay,
    PrependPrevious,
}

impl Field {
    pub(crate) const ALL: &'static [Field] = &[
        Field::MpvPath,
        Field::MpvArgs,
        Field::LogLevel,
        Field::Autoplay,
        Field::PrependPrevious,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Field::MpvPath => "mpv_path",
            Field::MpvArgs => "mpv_args",
            Field::LogLevel => "log_level",
            Field::Autoplay => "autoplay",
            Field::PrependPrevious => "prepend_previous",
        }
    }

    pub(crate) fn parse(key: &str) -> color_eyre::Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|f| f.name() == key)
            .ok_or_else(|| {
                let known: Vec<&str> = Self::ALL.iter().map(|f| f.name()).collect();
                usage_err(format!(
                    "unknown config key {key:?} (valid: {})",
                    known.join(", ")
                ))
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub(crate) mpv_path: String,
    pub log_level: String,
    pub(crate) autoplay: bool,
    pub(crate) prepend_previous: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mpv_path: "mpv".into(),
            log_level: "info".into(),
            autoplay: true,
            prepend_previous: true,
        }
    }
}

impl Config {
    /// Reads config.toml, or the defaults when there is none. Pure;
    /// [`Self::load_or_create`] is the version that writes.
    pub fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let path = paths.config_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        let cfg: Self =
            toml::from_str(&text).wrap_err_with(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// For the daemon, which is a reasonable moment to materialise a config
    /// file the user can then edit by hand.
    pub(crate) fn load_or_create(paths: &Paths) -> color_eyre::Result<Self> {
        let cfg = Self::load(paths)?;
        if !paths.config_file().exists() {
            cfg.save(paths)?;
        }
        Ok(cfg)
    }

    pub(crate) fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        let text = toml::to_string_pretty(self).wrap_err("serializing config.toml")?;
        atomic_write(&paths.config_file(), text.as_bytes(), 0o644)?;
        Ok(())
    }

    /// `None` for [`Field::MpvArgs`], which is not in config.toml — the caller
    /// reads it from `mpv_args.conf` instead.
    pub(crate) fn get(&self, field: Field) -> Option<String> {
        match field {
            Field::MpvArgs => None,
            Field::MpvPath => Some(self.mpv_path.clone()),
            Field::LogLevel => Some(self.log_level.clone()),
            Field::Autoplay => Some(self.autoplay.to_string()),
            Field::PrependPrevious => Some(self.prepend_previous.to_string()),
        }
    }

    pub(crate) fn to_toml(&self) -> color_eyre::Result<String> {
        toml::to_string_pretty(self).wrap_err("serializing config.toml")
    }

    /// Returns `false` for [`Field::MpvArgs`], which the caller writes to its
    /// own file.
    pub(crate) fn set(&mut self, field: Field, value: &str) -> color_eyre::Result<bool> {
        match field {
            Field::MpvArgs => return Ok(false),
            Field::MpvPath => self.mpv_path = value.to_string(),
            // Rejected here rather than at the next startup.
            Field::LogLevel => {
                crate::app::tracing::validate_log_level(value)
                    .map_err(|e| usage_err(format!("invalid log_level {value:?}: {e}")))?;
                self.log_level = value.to_string();
            }
            Field::Autoplay => self.autoplay = parse_bool(value)?,
            Field::PrependPrevious => self.prepend_previous = parse_bool(value)?,
        }
        Ok(true)
    }
}

/// Extra mpv argv, kept in its own file and re-read on every spawn.
///
/// One argument per line (`--title=My Movie` is one line, not two words);
/// blank lines and `#` comments are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MpvArgs(pub(crate) Vec<String>);

impl MpvArgs {
    pub(crate) fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let path = paths.mpv_args_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        Ok(Self(parse_mpv_args(&text)))
    }

    pub(crate) fn save(paths: &Paths, value: &str) -> color_eyre::Result<()> {
        let args = parse_mpv_args(value);
        let mut text = String::new();
        for arg in &args {
            text.push_str(arg);
            text.push('\n');
        }
        paths.ensure()?;
        atomic_write(&paths.mpv_args_file(), text.as_bytes(), 0o644)?;
        Ok(())
    }

    pub(crate) fn get(paths: &Paths) -> color_eyre::Result<String> {
        Ok(Self::load(paths)?.0.join(" "))
    }
}

/// Split a value into mpv arguments. Whitespace-separated, like a shell
/// command line without quoting.
fn parse_mpv_args(value: &str) -> Vec<String> {
    value
        .lines()
        .flat_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                Vec::new()
            } else {
                line.split_whitespace().map(str::to_string).collect()
            }
        })
        .collect()
}

fn parse_bool(value: &str) -> color_eyre::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(usage_err(format!(
            "invalid boolean {value:?}; use true/false"
        ))),
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Credentials {
    pub(crate) server: String,
    pub(crate) username: String,
    pub(crate) user_id: String,
    pub(crate) access_token: String,
    pub(crate) device_id: String,
}

impl fmt::Debug for Credentials {
    /// Hand-written so `access_token` cannot reach a log line. Serialization is
    /// unaffected — cred.json still holds the token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("user_id", &self.user_id)
            .field("access_token", &"<redacted>")
            .field("device_id", &self.device_id)
            .finish()
    }
}

impl Credentials {
    pub(crate) fn load(paths: &Paths) -> color_eyre::Result<Option<Self>> {
        let path = paths.cred_file();
        if !path.exists() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        let creds: Self =
            serde_json::from_str(&text).wrap_err_with(|| format!("parsing {}", path.display()))?;
        Ok(Some(creds))
    }

    pub(crate) fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        let text = serde_json::to_string_pretty(self).wrap_err("serializing cred.json")?;
        atomic_write(&paths.cred_file(), text.as_bytes(), 0o600)?;
        Ok(())
    }

    pub(crate) fn remove(paths: &Paths) -> color_eyre::Result<()> {
        let path = paths.cred_file();
        if path.exists() {
            fs::remove_file(&path).wrap_err_with(|| format!("removing {}", path.display()))?;
        }
        Ok(())
    }
}

fn atomic_write(path: &Path, data: &[u8], mode: u32) -> color_eyre::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f =
            fs::File::create(&tmp).wrap_err_with(|| format!("creating {}", tmp.display()))?;
        f.write_all(data)
            .wrap_err_with(|| format!("writing {}", tmp.display()))?;
        f.sync_all()
            .wrap_err_with(|| format!("flushing {}", tmp.display()))?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
        .wrap_err_with(|| format!("restricting {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .wrap_err_with(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Bare host → `http://host:8096`. Existing scheme/port/path are kept.
/// Trailing slashes are stripped.
pub(crate) fn normalize_server_url(input: &str) -> color_eyre::Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(usage_err("server URL is empty"));
    }

    if !trimmed.contains("://") {
        let first = trimmed.split('/').next().unwrap_or_default();
        if matches!(
            first.to_ascii_lowercase().as_str(),
            "http" | "https" | "http:" | "https:"
        ) {
            return Err(usage_err(
                "scheme is missing '//' — expected e.g. 'http://host:8096'",
            ));
        }
    }

    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    let url = reqwest::Url::parse(&with_scheme)
        .wrap_err_with(|| format!("invalid server URL {input:?}"))?;

    let host = url
        .host_str()
        .ok_or_else(|| usage_err("server URL has no host"))?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    let port_part = match url.port() {
        Some(p) => format!(":{p}"),
        None if explicit_port(trimmed) => {
            // `Url::port()` hides 80/443, but the user wrote it on purpose.
            match url.port_or_known_default() {
                Some(p) => format!(":{p}"),
                None => String::new(),
            }
        }
        None if url.scheme() == "http" => ":8096".to_string(),
        None => String::new(),
    };

    let path = url.path().trim_end_matches('/');
    let path = if path.is_empty() || path == "/" {
        String::new()
    } else {
        path.to_string()
    };

    Ok(format!("{}://{}{}{}", url.scheme(), host, port_part, path))
}

fn explicit_port(input: &str) -> bool {
    let rest = match input.split_once("://") {
        Some((_, r)) => r,
        None => input,
    };
    if let Some(end) = rest.find(']') {
        return rest[end + 1..].starts_with(':');
    }
    let hostport = rest.split('/').next().unwrap_or(rest);
    hostport.contains(':')
}

pub(crate) fn device_name() -> String {
    let name = rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        APP_NAME.to_string()
    } else {
        name
    }
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
