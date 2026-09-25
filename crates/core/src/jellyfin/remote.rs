use super::auth::Api;
use super::model::Session;
use super::url::encode_query_value;
use color_eyre::eyre::{Result, WrapErr};
use serde::Deserialize;

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
        self.post(&path).await
    }
}

#[cfg(test)]
#[path = "remote_test.rs"]
mod tests;
