use super::{MpvArgs, Paths, atomic_write, read_optional};
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};

/// Every user-facing configuration key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    MpvPath,
    /// Lives in `mpv_args.conf`, re-read on every mpv spawn.
    MpvArgs,
    Autoplay,
    PrependPrevious,
    CoverCacheMb,
}

impl Field {
    pub const ALL: &'static [Field] = &[
        Field::MpvPath,
        Field::MpvArgs,
        Field::Autoplay,
        Field::PrependPrevious,
        Field::CoverCacheMb,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Field::MpvPath => "mpv_path",
            Field::MpvArgs => "mpv_args",
            Field::Autoplay => "autoplay",
            Field::PrependPrevious => "prepend_previous",
            Field::CoverCacheMb => "cover_cache_mb",
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

    pub fn read(self, paths: &Paths) -> color_eyre::Result<String> {
        let config = Config::load(paths)?;
        Ok(match self {
            Field::MpvPath => config.mpv_path,
            Field::MpvArgs => MpvArgs::load(paths)?.0.join(" "),
            Field::Autoplay => config.autoplay.to_string(),
            Field::PrependPrevious => config.prepend_previous.to_string(),
            Field::CoverCacheMb => config.cover_cache_mb.to_string(),
        })
    }

    pub fn write(self, paths: &Paths, value: &str) -> color_eyre::Result<()> {
        let mut config = Config::load(paths)?;
        match self {
            Field::MpvPath => config.mpv_path = value.to_string(),
            Field::MpvArgs => return MpvArgs::save(paths, value),
            Field::Autoplay => config.autoplay = parse_bool(value)?,
            Field::PrependPrevious => config.prepend_previous = parse_bool(value)?,
            Field::CoverCacheMb => {
                config.cover_cache_mb = value.trim().parse().map_err(|_| {
                    usage_err(format!("invalid cover_cache_mb {value:?}; use a whole number of megabytes, or 0 to turn the cache off"))
                })?;
            }
        }
        config.save(paths)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub mpv_path: String,
    pub autoplay: bool,
    pub prepend_previous: bool,
    pub cover_cache_mb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mpv_path: "mpv".into(),
            autoplay: true,
            prepend_previous: true,
            cover_cache_mb: 256,
        }
    }
}

impl Config {
    /// Reads config.toml, or the defaults when there is none.
    pub fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let path = paths.config_file();
        let Some(text) = read_optional(&path)? else {
            return Ok(Self::default());
        };
        toml::from_str(&text).wrap_err_with(|| format!("parsing {}", path.display()))
    }

    pub fn load_or_create(paths: &Paths) -> color_eyre::Result<Self> {
        let cfg = Self::load(paths)?;
        if !paths.config_file().exists() {
            cfg.save(paths)?;
        }
        Ok(cfg)
    }

    pub(crate) fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        atomic_write(&paths.config_file(), self.to_toml()?.as_bytes(), 0o644)
    }

    pub fn to_toml(&self) -> color_eyre::Result<String> {
        toml::to_string_pretty(self).wrap_err("serializing config.toml")
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
