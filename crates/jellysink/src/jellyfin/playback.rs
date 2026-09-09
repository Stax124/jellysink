use super::profile::{capabilities, device_profile};
use crate::media::PlayRequest;
use crate::report::PlayingState;
use color_eyre::eyre::WrapErr;
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::jellyfin::encode_query_value;
use serde_json::{Value, json};

pub(crate) async fn post_capabilities(api: &Api) -> color_eyre::Result<()> {
    api.post_json("/Sessions/Capabilities/Full", &capabilities())
        .await?
        .error_for_status()
        .wrap_err("posting session capabilities")?;
    Ok(())
}

pub(crate) async fn playback_info(
    api: &Api,
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
        "UserId": api.user_id,
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
        encode_query_value(&api.user_id)
    );
    let resp = api
        .post_json(&path, &body)
        .await?
        .error_for_status()
        .wrap_err("PlaybackInfo")?;
    resp.json().await.wrap_err("decoding PlaybackInfo")
}

pub(crate) async fn playing(api: &Api, state: &PlayingState) -> color_eyre::Result<()> {
    post_session(api, "/Sessions/Playing", state).await
}

pub(crate) async fn progress(api: &Api, state: &PlayingState) -> color_eyre::Result<()> {
    post_session(api, "/Sessions/Playing/Progress", state).await
}

pub(crate) async fn stopped(api: &Api, state: &PlayingState) -> color_eyre::Result<()> {
    post_session(api, "/Sessions/Playing/Stopped", state).await
}

async fn post_session(api: &Api, path: &str, state: &PlayingState) -> color_eyre::Result<()> {
    let body = state.to_json();
    let resp = api.post_json(path, &body).await?;
    if !resp.status().is_success() {
        tracing::debug!(status = %resp.status(), path, "session report rejected");
    }
    Ok(())
}
