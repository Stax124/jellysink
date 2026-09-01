use super::auth::Api;
use super::profile::{capabilities, device_profile};
use crate::report::PlayingState;
use color_eyre::eyre::WrapErr;
use serde_json::{Value, json};

pub struct PlaybackEndpoints<'a> {
    pub api: &'a Api,
}

impl<'a> PlaybackEndpoints<'a> {
    pub fn new(api: &'a Api) -> Self {
        Self { api }
    }

    pub async fn post_capabilities(&self) -> color_eyre::Result<()> {
        self.api
            .post_json("/Sessions/Capabilities/Full", &capabilities())
            .await?
            .error_for_status()
            .wrap_err("posting session capabilities")?;
        Ok(())
    }

    pub async fn playback_info(
        &self,
        item_id: &str,
        start_ticks: Option<i64>,
        aid: Option<i64>,
        sid: Option<i64>,
        media_source_id: Option<&str>,
    ) -> color_eyre::Result<Value> {
        let mut body = json!({
            "DeviceProfile": device_profile(),
            "UserId": self.api.user_id,
            "StartTimeTicks": start_ticks.unwrap_or(0),
            "IsPlayback": true,
            "AutoOpenLiveStream": true,
            "MaxStreamingBitrate": 1_200_000_000u64,
        });
        if let Some(aid) = aid {
            body["AudioStreamIndex"] = json!(aid);
        }
        if let Some(sid) = sid {
            body["SubtitleStreamIndex"] = json!(sid);
        }
        if let Some(src) = media_source_id {
            body["MediaSourceId"] = json!(src);
        }
        let path = format!("/Items/{item_id}/PlaybackInfo?UserId={}", self.api.user_id);
        let resp = self
            .api
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
    pub async fn episodes_all(&self, series_id: &str) -> color_eyre::Result<Value> {
        let path = format!(
            "/Shows/{series_id}/Episodes?userId={}&Limit=500",
            self.api.user_id
        );
        tracing::debug!(path, "GET all episodes");
        self.get_json(&path).await
    }

    pub async fn get_item(&self, item_id: &str) -> color_eyre::Result<Value> {
        let path = format!("/Items/{item_id}?userId={}", self.api.user_id);
        match self.get_json(&path).await {
            Ok(v) => Ok(v),
            Err(_) => {
                let legacy = format!("/Users/{}/Items/{item_id}", self.api.user_id);
                self.get_json(&legacy).await
            }
        }
    }

    async fn get_json(&self, path: &str) -> color_eyre::Result<Value> {
        let resp = self
            .api
            .get(path)
            .await?
            .error_for_status()
            .wrap_err_with(|| format!("GET {path}"))?;
        resp.json().await.wrap_err("decoding JSON")
    }

    pub async fn playing(&self, state: &PlayingState) -> color_eyre::Result<()> {
        self.post_session("/Sessions/Playing", state).await
    }

    pub async fn progress(&self, state: &PlayingState) -> color_eyre::Result<()> {
        self.post_session("/Sessions/Playing/Progress", state).await
    }

    pub async fn stopped(&self, state: &PlayingState) -> color_eyre::Result<()> {
        self.post_session("/Sessions/Playing/Stopped", state).await
    }

    async fn post_session(&self, path: &str, state: &PlayingState) -> color_eyre::Result<()> {
        let body = state.to_json();
        let resp = self.api.post_json(path, &body).await?;
        if !resp.status().is_success() {
            tracing::debug!(status = %resp.status(), path, "session report rejected");
        }
        Ok(())
    }
}
