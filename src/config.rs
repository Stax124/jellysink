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

    /// Creates the config directory, mode 0700.
    ///
    /// This is the race-free half of keeping the access token private. cred.json
    /// gets 0600 of its own, but `mpv.sock` is created by *mpv*, and anyone who
    /// can open it can read the token back out of `http-header-fields`. We
    /// cannot choose that socket's mode, so we make the directory around it
    /// unreadable to anyone else instead. Applied on every run, so an existing
    /// 0755 directory from an older version is tightened too.
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

/// Every user-facing configuration key.
///
/// The list used to be spelled out in six places — the struct, four `default_*`
/// functions, the `Default` impl, `get`, `set`, and the CLI help — with
/// `mpv_args` handled entirely outside `Config` as a seventh special case, so
/// `jellysink config get` with no key silently omitted it. Matching on this
/// enum is exhaustive, so adding a key is a compile error until every arm is
/// handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    MpvPath,
    /// Lives in `mpv_args.conf`, not `config.toml`: a running daemon re-reads
    /// it on every mpv spawn instead of holding a copy from startup.
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
    /// Reads config.toml, or the defaults when there is none.
    ///
    /// Pure: this used to `save` on a missing file, so `jellysink config path`
    /// created one as a side effect. [`Self::load_or_create`] is the version
    /// that writes.
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
            // Validate now rather than failing at the next startup, which is
            // where `parse_log_filter` would otherwise reject it.
            Field::LogLevel => {
                crate::tracing::validate_log_level(value)
                    .map_err(|e| usage_err(format!("invalid log_level {value:?}: {e}")))?;
                self.log_level = value.to_string();
            }
            Field::Autoplay => self.autoplay = parse_bool(value)?,
            Field::PrependPrevious => self.prepend_previous = parse_bool(value)?,
        }
        Ok(true)
    }
}

/// Extra mpv argv, kept in its own file so a running daemon re-reads it on
/// every spawn instead of holding a stale copy from startup.
///
/// One argument per line; blank lines and `#` comments are ignored. A line
/// may itself contain whitespace (e.g. `--title=My Movie`), so splitting is
/// by line, not by shell words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MpvArgs(pub Vec<String>);

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
    /// Hand-written so `access_token` cannot reach a log line or a color-eyre
    /// capture. Serialization is unaffected — cred.json still holds the token.
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
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
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
mod tests {
    use super::*;
    #[test]
    fn ensure_makes_the_config_dir_private() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("jellysink"),
        };
        paths.ensure().unwrap();
        let mode = fs::metadata(&paths.config_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "mpv.sock lives here and leaks the token");
    }

    #[test]
    fn ensure_tightens_a_directory_left_world_readable_by_an_older_version() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("jellysink"),
        };
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::set_permissions(&paths.config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        paths.ensure().unwrap();
        let mode = fs::metadata(&paths.config_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

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
        cfg.set(Field::MpvPath, "/usr/bin/mpv").unwrap();
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
    fn unknown_config_key_errors_and_names_the_valid_ones() {
        let err = Field::parse("nope").unwrap_err();
        assert!(
            err.downcast_ref::<crate::UsageError>().is_some(),
            "expected UsageError, got {err:?}"
        );
        let msg = err.to_string();
        for field in Field::ALL {
            assert!(msg.contains(field.name()), "{msg} should list {field:?}");
        }
    }

    #[test]
    fn every_field_parses_back_from_its_own_name() {
        for field in Field::ALL {
            assert_eq!(Field::parse(field.name()).unwrap(), *field);
        }
    }

    /// `--help` spells the key list out; keep it honest.
    #[test]
    fn the_cli_help_lists_every_config_key() {
        let main_rs = include_str!("main.rs");
        for field in Field::ALL {
            assert!(
                main_rs.contains(field.name()),
                "src/main.rs should mention {:?} in the `config set` help",
                field.name()
            );
        }
    }

    #[test]
    fn an_invalid_log_level_is_rejected_at_set_time() {
        let mut cfg = Config::default();
        // This used to be accepted and only fail at the next startup.
        let err = cfg.set(Field::LogLevel, "banana").unwrap_err();
        assert!(
            err.downcast_ref::<crate::UsageError>().is_some(),
            "expected UsageError, got {err:?}"
        );
        assert_eq!(cfg.log_level, "info", "the bad value must not be stored");
        cfg.set(Field::LogLevel, "jellysink=debug,warn").unwrap();
        assert_eq!(cfg.log_level, "jellysink=debug,warn");
    }

    #[test]
    fn mpv_args_is_not_stored_in_config_toml() {
        let mut cfg = Config::default();
        assert_eq!(cfg.get(Field::MpvArgs), None);
        assert!(
            !cfg.set(Field::MpvArgs, "--fullscreen").unwrap(),
            "the caller writes this to mpv_args.conf instead"
        );
    }

    #[test]
    fn load_does_not_create_a_config_file() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        let cfg = Config::load(&paths).unwrap();
        assert_eq!(cfg, Config::default());
        assert!(
            !paths.config_file().exists(),
            "`jellysink config path` should not write a config file"
        );
        Config::load_or_create(&paths).unwrap();
        assert!(paths.config_file().exists());
    }

    #[test]
    fn invalid_autoplay_value_is_a_usage_error() {
        let mut cfg = Config::default();
        let err = cfg.set(Field::Autoplay, "maybe").unwrap_err();
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
        cfg.set(Field::Autoplay, "false").unwrap();
        cfg.save(&paths).unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert!(!loaded.autoplay);
        assert_eq!(loaded.get(Field::Autoplay).as_deref(), Some("false"));
    }

    #[test]
    fn credentials_debug_never_prints_the_access_token() {
        let creds = Credentials {
            server: "http://s".into(),
            username: "u".into(),
            user_id: "uid".into(),
            access_token: "sekrit".into(),
            device_id: "d".into(),
        };
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("sekrit"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }
}
