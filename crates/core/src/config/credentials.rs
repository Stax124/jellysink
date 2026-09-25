use super::{Paths, atomic_write, read_optional};
use crate::usage_err;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::ErrorKind;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    pub server: String,
    pub username: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
}

/// Hand-written so `access_token` cannot reach a log line
impl fmt::Debug for Credentials {
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
    pub fn load(paths: &Paths) -> color_eyre::Result<Option<Self>> {
        let path = paths.cred_file();
        let Some(text) = read_optional(&path)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .wrap_err_with(|| format!("parsing {}", path.display()))
    }

    pub fn load_required(paths: &Paths) -> color_eyre::Result<Self> {
        Self::load(paths)?.ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))
    }

    pub fn save(&self, paths: &Paths) -> color_eyre::Result<()> {
        paths.ensure()?;
        let text = serde_json::to_string_pretty(self).wrap_err("serializing cred.json")?;
        atomic_write(&paths.cred_file(), text.as_bytes(), 0o600)
    }

    pub fn remove(paths: &Paths) -> color_eyre::Result<()> {
        let path = paths.cred_file();
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).wrap_err_with(|| format!("removing {}", path.display())),
        }
    }
}

#[cfg(test)]
#[path = "credentials_test.rs"]
mod tests;
