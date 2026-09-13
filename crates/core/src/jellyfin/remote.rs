//! Driving another Jellyfin session — here, the jellysink daemon. Every call
//! lands in [`crate::cast`], so the names must match `CastEvent::from_ws`.

use super::auth::Api;
use super::encode_query_value;
use super::model::Session;
use color_eyre::eyre::{Result, WrapErr};
use serde::Deserialize;

/// The `PlayCommand` value [`Api::play_now`] sends. Named so the round-trip
/// test can hold it against what `cast.rs` parses — a typo fails at runtime.
const PLAY_NOW: &str = "PlayNow";

impl Api {
    /// jellysink's own session, or `None` when the daemon is not connected.
    /// Filtered by device id: an earlier install leaves a same-named session
    /// behind that commands go nowhere through.
    pub async fn session_for_device(&self) -> Result<Option<Session>> {
        let path = format!("/Sessions?deviceId={}", encode_query_value(&self.device_id));
        let body = self.get_json(&path).await?;
        let sessions = Vec::<Session>::deserialize(&body).wrap_err("decoding Sessions")?;
        Ok(sessions.into_iter().next())
    }

    pub async fn play_now(&self, session_id: &str, item_id: &str, start_ticks: i64) -> Result<()> {
        let path = format!(
            "/Sessions/{}/Playing?PlayCommand={PLAY_NOW}&ItemIds={}&StartPositionTicks={start_ticks}",
            encode_query_value(session_id),
            encode_query_value(item_id),
        );
        tracing::debug!(item_id, start_ticks, "PlayNow");
        self.post_command(&path).await
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
