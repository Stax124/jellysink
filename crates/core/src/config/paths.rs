use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    cache_dir: PathBuf,
}

impl Paths {
    /// An overridden config directory takes the cache with it, so `--config`
    /// isolates a run rather than leaving it writing to the real `~/.cache`.
    pub fn from_override(config_dir: Option<PathBuf>) -> color_eyre::Result<Self> {
        let Some(config_dir) = config_dir else {
            let dirs = directories::ProjectDirs::from("", APP_NAME, APP_NAME)
                .ok_or_else(|| usage_err("could not resolve a config directory"))?;
            return Ok(Self {
                config_dir: dirs.config_dir().to_path_buf(),
                cache_dir: dirs.cache_dir().to_path_buf(),
            });
        };
        let cache_dir = config_dir.join("cache");
        Ok(Self {
            config_dir,
            cache_dir,
        })
    }

    /// Creates the config directory, mode 0700: `mpv.sock` is created by mpv
    /// under its own umask and hands out the access token.
    pub fn ensure(&self) -> color_eyre::Result<()> {
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

    pub fn stop_socket(&self) -> PathBuf {
        self.config_dir.join("stop.sock")
    }

    pub fn mpv_socket(&self) -> PathBuf {
        self.config_dir.join("mpv.sock")
    }

    pub(crate) fn mpv_args_file(&self) -> PathBuf {
        self.config_dir.join("mpv_args.conf")
    }

    pub fn cover_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("covers")
    }
}

#[cfg(test)]
#[path = "paths_test.rs"]
mod tests;
