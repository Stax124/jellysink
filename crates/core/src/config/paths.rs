use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

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

    /// Creates the config directory, mode 0700 — `mpv.sock` is created by mpv
    /// under its own umask and hands out the access token, so the directory
    /// around it is what keeps it private. Re-applied on every run.
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
}

#[cfg(test)]
#[path = "paths_test.rs"]
mod tests;
