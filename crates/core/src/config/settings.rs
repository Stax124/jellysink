use super::Paths;
use super::atomic_write;
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::fs;

/// Every user-facing configuration key. Matching on it is exhaustive, so a new
/// key is a compile error until every place that handles keys handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    MpvPath,
    /// Lives in `mpv_args.conf`, re-read on every mpv spawn.
    MpvArgs,
    LogLevel,
    Autoplay,
    PrependPrevious,
}

impl Field {
    pub const ALL: &'static [Field] = &[
        Field::MpvPath,
        Field::MpvArgs,
        Field::LogLevel,
        Field::Autoplay,
        Field::PrependPrevious,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Field::MpvPath => "mpv_path",
            Field::MpvArgs => "mpv_args",
            Field::LogLevel => "log_level",
            Field::Autoplay => "autoplay",
            Field::PrependPrevious => "prepend_previous",
        }
    }

    pub fn parse(key: &str) -> color_eyre::Result<Self> {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub mpv_path: String,
    pub log_level: String,
    pub autoplay: bool,
    pub prepend_previous: bool,
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

    /// The log level to start tracing with, for the two binaries that must do
    /// so before a config error has anywhere to be reported.
    pub fn configured_log_level(paths: &Paths) -> String {
        Self::load(paths).map_or_else(|_| Self::default().log_level, |cfg| cfg.log_level)
    }

    /// For the daemon, which is a reasonable moment to materialise a config
    /// file the user can then edit by hand.
    pub fn load_or_create(paths: &Paths) -> color_eyre::Result<Self> {
        let cfg = Self::load(paths)?;
        if !paths.config_file().exists() {
            cfg.save(paths)?;
        }
        Ok(cfg)
    }

    pub fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        let text = toml::to_string_pretty(self).wrap_err("serializing config.toml")?;
        atomic_write(&paths.config_file(), text.as_bytes(), 0o644)?;
        Ok(())
    }

    /// `None` for [`Field::MpvArgs`], which is not in config.toml — the caller
    /// reads it from `mpv_args.conf` instead.
    pub fn get(&self, field: Field) -> Option<String> {
        match field {
            Field::MpvArgs => None,
            Field::MpvPath => Some(self.mpv_path.clone()),
            Field::LogLevel => Some(self.log_level.clone()),
            Field::Autoplay => Some(self.autoplay.to_string()),
            Field::PrependPrevious => Some(self.prepend_previous.to_string()),
        }
    }

    pub fn to_toml(&self) -> color_eyre::Result<String> {
        toml::to_string_pretty(self).wrap_err("serializing config.toml")
    }

    /// Returns `false` for [`Field::MpvArgs`], which the caller writes to its
    /// own file.
    pub fn set(&mut self, field: Field, value: &str) -> color_eyre::Result<bool> {
        match field {
            Field::MpvArgs => return Ok(false),
            Field::MpvPath => self.mpv_path = value.to_string(),
            // Rejected here rather than at the next startup.
            Field::LogLevel => {
                crate::logging::validate_log_level(value)
                    .map_err(|e| usage_err(format!("invalid log_level {value:?}: {e}")))?;
                self.log_level = value.to_string();
            }
            Field::Autoplay => self.autoplay = parse_bool(value)?,
            Field::PrependPrevious => self.prepend_previous = parse_bool(value)?,
        }
        Ok(true)
    }
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

#[cfg(test)]
#[path = "settings_test.rs"]
mod tests;
