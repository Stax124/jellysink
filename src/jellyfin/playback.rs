use super::auth::Api;
use super::encode_query_value;
use super::profile::{capabilities, device_profile};
use crate::media::PlayRequest;
use crate::report::PlayingState;
use color_eyre::eyre::WrapErr;
use serde_json::{Value, json};

/// The playback and session endpoints.
///
/// These used to hang off a `PlaybackEndpoints<'a>` wrapper holding a single
/// `&Api` field, constructed at four call sites and adding nothing.
impl Api {
    pub(crate) async fn post_capabilities(&self) -> color_eyre::Result<()> {
        self.post_json("/Sessions/Capabilities/Full", &capabilities())
            .await?
            .error_for_status()
            .wrap_err("posting session capabilities")?;
        Ok(())
    }

    pub(crate) async fn playback_info(
        &self,
        item_id: &str,
        req: &PlayRequest,
    ) -> color_eyre::Result<Value> {
        let PlayRequest {
            start_ticks,
            audio_stream_index,
            subtitle_stream_index,
            media_source_id,
        } = req;
        let (start_ticks, audio_stream_index, subtitle_stream_index) =
            (*start_ticks, *audio_stream_index, *subtitle_stream_index);
        let mut body = json!({
            "DeviceProfile": device_profile(),
            "UserId": self.user_id,
            "StartTimeTicks": start_ticks.unwrap_or(0),
            "IsPlayback": true,
            "AutoOpenLiveStream": true,
            "MaxStreamingBitrate": 1_200_000_000u64,
        });
        if let Some(audio_stream_index) = audio_stream_index {
            body["AudioStreamIndex"] = json!(audio_stream_index);
        }
        if let Some(subtitle_stream_index) = subtitle_stream_index {
            body["SubtitleStreamIndex"] = json!(subtitle_stream_index);
        }
        if let Some(media_source_id) = media_source_id {
            body["MediaSourceId"] = json!(media_source_id);
        }
        let path = format!(
            "/Items/{item_id}/PlaybackInfo?UserId={}",
            encode_query_value(&self.user_id)
        );
        let resp = self
            .post_json(&path, &body)
            .await?
            .error_for_status()
            .wrap_err("PlaybackInfo")?;
        resp.json().await.wrap_err("decoding PlaybackInfo")
    }

    /// The whole series in aired order, with no `StartItemId` cursor.
    ///
    /// `StartItemId` is a forward-only `SkipWhile`, so it can never return the
    /// episodes *before* the current one. Omitting it is the only way to see
    /// them; the caller splits the listing at the current item.
    pub(crate) async fn episodes_all(&self, series_id: &str) -> color_eyre::Result<Value> {
        let path = format!(
            "/Shows/{series_id}/Episodes?userId={}&Limit=500",
            encode_query_value(&self.user_id)
        );
        tracing::debug!(path, "GET all episodes");
        self.get_json(&path).await
    }

    pub(crate) async fn get_item(&self, item_id: &str) -> color_eyre::Result<Value> {
        let path = format!(
            "/Items/{item_id}?userId={}",
            encode_query_value(&self.user_id)
        );
        match self.get_json(&path).await {
            Ok(v) => Ok(v),
            Err(_) => {
                let legacy = format!("/Users/{}/Items/{item_id}", self.user_id);
                self.get_json(&legacy).await
            }
        }
    }

    async fn get_json(&self, path: &str) -> color_eyre::Result<Value> {
        let resp = self
            .get(path)
            .await?
            .error_for_status()
            .wrap_err_with(|| format!("GET {path}"))?;
        resp.json().await.wrap_err("decoding JSON")
    }

    pub(crate) async fn playing(&self, state: &PlayingState) -> color_eyre::Result<()> {
        self.post_session("/Sessions/Playing", state).await
    }

    pub(crate) async fn progress(&self, state: &PlayingState) -> color_eyre::Result<()> {
        self.post_session("/Sessions/Playing/Progress", state).await
    }

    pub(crate) async fn stopped(&self, state: &PlayingState) -> color_eyre::Result<()> {
        self.post_session("/Sessions/Playing/Stopped", state).await
    }

    async fn post_session(&self, path: &str, state: &PlayingState) -> color_eyre::Result<()> {
        let body = state.to_json();
        let resp = self.post_json(path, &body).await?;
        if !resp.status().is_success() {
            tracing::debug!(status = %resp.status(), path, "session report rejected");
        }
        Ok(())
    }
}
