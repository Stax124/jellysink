//! Driving another Jellyfin session — here, the jellysink daemon.
//!
//! Every call lands in [`crate::cast`] on the other side, so the command names
//! and parameter spellings must match what `CastEvent::from_ws` parses.

use super::auth::Api;
use super::encode_query_value;
use super::model::Session;
use color_eyre::eyre::{Result, WrapErr};
use serde::Deserialize;
use serde_json::{Value, json};

/// The `Playstate` commands jellysink acts on. An enum so a caller cannot
/// invent a spelling the daemon silently drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaystateCommand {
    PlayPause,
    Stop,
    NextTrack,
    PreviousTrack,
    Seek,
}

impl PlaystateCommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::PlayPause => "PlayPause",
            Self::Stop => "Stop",
            Self::NextTrack => "NextTrack",
            Self::PreviousTrack => "PreviousTrack",
            Self::Seek => "Seek",
        }
    }
}

impl Api {
    /// jellysink's own session, or `None` when the daemon is not connected.
    /// Filtered by device id: an earlier install leaves a same-named session
    /// behind, and commands sent to that one go nowhere.
    pub async fn session_for_device(&self) -> Result<Option<Session>> {
        let path = format!("/Sessions?deviceId={}", encode_query_value(&self.device_id));
        let body = self.get_json(&path).await?;
        let sessions = Vec::<Session>::deserialize(&body).wrap_err("decoding Sessions")?;
        Ok(sessions.into_iter().next())
    }

    pub async fn play_now(&self, session_id: &str, item_id: &str, start_ticks: i64) -> Result<()> {
        let path = format!(
            "/Sessions/{}/Playing?PlayCommand=PlayNow&ItemIds={}&StartPositionTicks={start_ticks}",
            encode_query_value(session_id),
            encode_query_value(item_id),
        );
        tracing::debug!(item_id, start_ticks, "PlayNow");
        self.post_command(&path).await
    }

    pub async fn playstate(
        &self,
        session_id: &str,
        command: PlaystateCommand,
        seek_ticks: Option<i64>,
    ) -> Result<()> {
        let mut path = format!(
            "/Sessions/{}/Playing/{}",
            encode_query_value(session_id),
            command.as_str()
        );
        if let Some(seek_ticks) = seek_ticks {
            path.push_str(&format!("?SeekPositionTicks={seek_ticks}"));
        }
        self.post_command(&path).await
    }

    pub async fn general_command(
        &self,
        session_id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<()> {
        let path = format!("/Sessions/{}/Command", encode_query_value(session_id));
        let body = json!({ "Name": name, "Arguments": arguments });
        let resp = self.post_json(&path, &body).await?;
        resp.error_for_status()
            .wrap_err_with(|| format!("GeneralCommand {name}"))?;
        Ok(())
    }

    async fn post_command(&self, path: &str) -> Result<()> {
        self.post(path)
            .await?
            .error_for_status()
            .wrap_err_with(|| format!("POST {path}"))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "remote_test.rs"]
mod tests;
