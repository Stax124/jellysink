use super::Paths;
use super::atomic_write;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    pub server: String,
    pub username: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
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

#[cfg(test)]
#[path = "credentials_test.rs"]
mod tests;
