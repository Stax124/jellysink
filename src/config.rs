use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
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

    pub fn ensure(&self) -> color_eyre::Result<()> {
        fs::create_dir_all(&self.config_dir)
            .wrap_err_with(|| format!("creating {}", self.config_dir.display()))?;
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn cred_file(&self) -> PathBuf {
        self.config_dir.join("cred.json")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.config_dir.join("instance.lock")
    }

    pub fn stop_socket(&self) -> PathBuf {
        self.config_dir.join("stop.sock")
    }

    pub fn mpv_socket(&self) -> PathBuf {
        self.config_dir.join("mpv.sock")
    }

    pub fn mpv_args_file(&self) -> PathBuf {
        self.config_dir.join("mpv_args.conf")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default = "default_mpv_path")]
    pub mpv_path: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_autoplay")]
    pub autoplay: bool,
}

fn default_mpv_path() -> String {
    "mpv".into()
}

fn default_log_level() -> String {
    "info".into()
}

fn default_autoplay() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mpv_path: default_mpv_path(),
            log_level: default_log_level(),
            autoplay: default_autoplay(),
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let path = paths.config_file();
        if !path.exists() {
            let cfg = Self::default();
            cfg.save(paths)?;
            return Ok(cfg);
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        let cfg: Self =
            toml::from_str(&text).wrap_err_with(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        let text = toml::to_string_pretty(self).wrap_err("serializing config.toml")?;
        atomic_write(&paths.config_file(), text.as_bytes(), 0o644)?;
        Ok(())
    }

    pub fn get(&self, key: Option<&str>) -> color_eyre::Result<String> {
        match key {
            None => Ok(toml::to_string_pretty(self)?),
            Some("mpv_path") => Ok(self.mpv_path.clone()),
            Some("log_level") => Ok(self.log_level.clone()),
            Some("autoplay") => Ok(self.autoplay.to_string()),
            Some(other) => Err(usage_err(format!("unknown config key {other:?}"))),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> color_eyre::Result<()> {
        match key {
            "mpv_path" => self.mpv_path = value.to_string(),
            "log_level" => self.log_level = value.to_string(),
            "autoplay" => {
                self.autoplay = parse_bool(value)?;
            }
            other => return Err(usage_err(format!("unknown config key {other:?}"))),
        }
        Ok(())
    }
}

/// Extra mpv argv, kept in its own file so a running daemon re-reads it on
/// every spawn instead of holding a stale copy from startup.
///
/// One argument per line; blank lines and `#` comments are ignored. A line
/// may itself contain whitespace (e.g. `--title=My Movie`), so splitting is
/// by line, not by shell words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MpvArgs(pub Vec<String>);

impl MpvArgs {
    pub fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let path = paths.mpv_args_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        Ok(Self(parse_mpv_args(&text)))
    }

    pub fn save(paths: &Paths, value: &str) -> color_eyre::Result<()> {
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

    pub fn get(paths: &Paths) -> color_eyre::Result<String> {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    pub server: String,
    pub username: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
}

impl Credentials {
    pub fn load(paths: &Paths) -> color_eyre::Result<Option<Self>> {
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

    pub fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        let text = serde_json::to_string_pretty(self).wrap_err("serializing cred.json")?;
        atomic_write(&paths.cred_file(), text.as_bytes(), 0o600)?;
        Ok(())
    }

    pub fn remove(paths: &Paths) -> color_eyre::Result<()> {
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
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path)
        .wrap_err_with(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Bare host → `http://host:8096`. Existing scheme/port/path are kept.
/// Trailing slashes are stripped.
pub fn normalize_server_url(input: &str) -> color_eyre::Result<String> {
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
            // `Url::port()` hides the scheme default (80/443), but the user
            // wrote it on purpose — keep it.
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

pub fn device_name() -> String {
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bare_host_gets_http_and_8096() {
        assert_eq!(
            normalize_server_url("192.168.1.10").unwrap(),
            "http://192.168.1.10:8096"
        );
    }

    #[test]
    fn explicit_port_80_is_kept() {
        assert_eq!(
            normalize_server_url("http://media.local:80").unwrap(),
            "http://media.local:80"
        );
    }

    #[test]
    fn https_without_port_is_not_given_8096() {
        assert_eq!(
            normalize_server_url("https://jellyfin.example").unwrap(),
            "https://jellyfin.example"
        );
    }

    #[test]
    fn subpath_is_kept() {
        assert_eq!(
            normalize_server_url("http://host:8096/jellyfin/").unwrap(),
            "http://host:8096/jellyfin"
        );
    }

    #[test]
    fn scheme_typo_without_slashes_is_rejected() {
        assert!(normalize_server_url("http//").is_err());
        assert!(normalize_server_url("http/media.local").is_err());
        assert!(normalize_server_url("https:").is_err());
        assert!(normalize_server_url("http").is_err());
    }

    #[test]
    fn cred_file_is_mode_600() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        let creds = Credentials {
            server: "http://h:8096".into(),
            username: "u".into(),
            user_id: "id".into(),
            access_token: "tok".into(),
            device_id: "dev".into(),
        };
        creds.save(&paths).unwrap();
        let mode = fs::metadata(paths.cred_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(Credentials::load(&paths).unwrap().unwrap(), creds);
    }

    #[test]
    fn config_roundtrip_and_set() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        let mut cfg = Config::default();
        cfg.set("mpv_path", "/usr/bin/mpv").unwrap();
        cfg.save(&paths).unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert_eq!(loaded.mpv_path, "/usr/bin/mpv");
    }

    #[test]
    fn mpv_args_roundtrip_and_reload() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        MpvArgs::save(&paths, "--hwdec=no --vo=gpu").unwrap();
        let loaded = MpvArgs::load(&paths).unwrap();
        assert_eq!(loaded.0, vec!["--hwdec=no", "--vo=gpu"]);
        assert_eq!(MpvArgs::get(&paths).unwrap(), "--hwdec=no --vo=gpu");

        // A running daemon re-reads the file; edits must be visible.
        MpvArgs::save(&paths, "--fullscreen").unwrap();
        assert_eq!(MpvArgs::load(&paths).unwrap().0, vec!["--fullscreen"]);
    }

    #[test]
    fn mpv_args_missing_file_is_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        assert_eq!(MpvArgs::load(&paths).unwrap().0, Vec::<String>::new());
    }

    #[test]
    fn mpv_args_comments_and_blank_lines_ignored() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        fs::write(
            paths.mpv_args_file(),
            "# comment\n\n--fullscreen\n  \n--volume=50\n",
        )
        .unwrap();
        assert_eq!(
            MpvArgs::load(&paths).unwrap().0,
            vec!["--fullscreen", "--volume=50"]
        );
    }

    #[test]
    fn unknown_config_key_errors() {
        let mut cfg = Config::default();
        let err = cfg.set("nope", "x").unwrap_err();
        assert!(
            err.downcast_ref::<crate::UsageError>().is_some(),
            "expected UsageError, got {err:?}"
        );
        let err = cfg.get(Some("nope")).unwrap_err();
        assert!(
            err.downcast_ref::<crate::UsageError>().is_some(),
            "expected UsageError, got {err:?}"
        );
    }

    #[test]
    fn invalid_autoplay_value_is_a_usage_error() {
        let mut cfg = Config::default();
        let err = cfg.set("autoplay", "maybe").unwrap_err();
        assert!(
            err.downcast_ref::<crate::UsageError>().is_some(),
            "expected UsageError, got {err:?}"
        );
        assert!(err.to_string().contains("true/false"));
    }

    #[test]
    fn missing_autoplay_key_defaults_on() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        paths.ensure().unwrap();
        fs::write(
            paths.config_file(),
            "mpv_path = \"mpv\"\nlog_level = \"info\"\n",
        )
        .unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert!(loaded.autoplay);
    }

    #[test]
    fn autoplay_defaults_on_and_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        let cfg = Config::default();
        assert!(cfg.autoplay);
        cfg.save(&paths).unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert!(loaded.autoplay);

        let mut cfg = loaded;
        cfg.set("autoplay", "false").unwrap();
        cfg.save(&paths).unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert!(!loaded.autoplay);
        assert_eq!(loaded.get(Some("autoplay")).unwrap(), "false");
    }
}
