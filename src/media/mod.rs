//! Turning a Jellyfin item into something mpv can play.

pub(crate) mod audio;
pub(crate) mod streams;
pub(crate) mod subtitle;
pub(crate) mod title;
pub(crate) mod track;

pub(crate) use audio::{AudioMemory, AudioPreference, resolve_audio_index};
pub(crate) use streams::{
    MediaSource, PlaybackInfo, StreamMaps, has_foreign_subtitle_host,
    jellyfin_embedded_audio_index, jellyfin_embedded_subtitle_index, map_streams,
    mpv_audio_track_id, mpv_embedded_subtitle_track_id,
};
pub(crate) use subtitle::{SubtitleMemory, SubtitlePreference, resolve_subtitle_index};
pub(crate) use title::{display_title, episode_titles, item_type, series_id};

use crate::jellyfin::url::{direct_stream_url, redact_api_key};
use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;
use serde_json::Value;
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreparedPlay {
    pub(crate) url: String,
    pub(crate) media_source_id: String,
    pub(crate) play_session_id: String,
    pub(crate) live_stream_id: Option<String>,
    pub(crate) maps: StreamMaps,
    pub(crate) audio_stream_index: Option<i64>,
    pub(crate) subtitle_stream_index: Option<i64>,
    pub(crate) uses_auth_header: bool,
    pub(crate) external_sub_urls: Vec<(i64, String)>,
    pub(crate) title: String,
}

impl fmt::Debug for PreparedPlay {
    /// Hand-written so `url` cannot carry the access token into a log line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedPlay")
            .field("url", &redact_api_key(&self.url))
            .field("media_source_id", &self.media_source_id)
            .field("play_session_id", &self.play_session_id)
            .field("live_stream_id", &self.live_stream_id)
            .field("maps", &self.maps)
            .field("audio_stream_index", &self.audio_stream_index)
            .field("subtitle_stream_index", &self.subtitle_stream_index)
            .field("uses_auth_header", &self.uses_auth_header)
            .field("external_sub_urls", &self.external_sub_urls)
            .field("title", &self.title)
            .finish()
    }
}

/// What the remote asked for when starting an item. A struct because three of
/// the four are `Option<i64>`, and most callers want [`PlayRequest::default`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PlayRequest {
    /// Resume offset, in Jellyfin ticks.
    pub(crate) start_ticks: Option<i64>,
    /// Jellyfin audio stream index the remote chose.
    pub(crate) audio_stream_index: Option<i64>,
    /// Jellyfin subtitle stream index. `Some(-1)` is an explicit "off";
    /// `None` means "whatever the server defaults to".
    pub(crate) subtitle_stream_index: Option<i64>,
    /// A specific version of a multi-version item.
    pub(crate) media_source_id: Option<String>,
}

impl PlayRequest {
    /// Whether this asks for anything beyond "just play it from the start",
    /// i.e. whether a cached `PreparedPlay` can be reused.
    pub(crate) fn is_plain(&self) -> bool {
        self.audio_stream_index.is_none()
            && self.subtitle_stream_index.is_none()
            && self.media_source_id.is_none()
            && self.start_ticks.unwrap_or(0) == 0
    }
}

pub(crate) fn prepare_play(
    server: &str,
    item_id: &str,
    playback_info: &Value,
    req: &PlayRequest,
    token: &str,
) -> color_eyre::Result<PreparedPlay> {
    let info = PlaybackInfo::deserialize(playback_info).wrap_err("decoding PlaybackInfo")?;

    let play_session_id = info
        .play_session_id
        .ok_or_else(|| eyre!("PlaybackInfo missing PlaySessionId"))?;

    let source = select_media_source(&info.media_sources, req.media_source_id.as_deref())
        .ok_or_else(|| eyre!("PlaybackInfo has no media sources"))?;

    if !source.supports_direct_play && !source.supports_direct_stream {
        return Err(eyre!(
            "server will not DirectPlay this item (SupportsDirectPlay=false, SupportsDirectStream=false); transcoding is disabled"
        ));
    }

    let media_source_id = source
        .id
        .clone()
        .ok_or_else(|| eyre!("MediaSource missing Id"))?;
    let live_stream_id = source.live_stream_id.clone();

    let maps = map_streams(server, source);
    let uses_auth_header = !has_foreign_subtitle_host(server, source);

    let url = direct_stream_url(
        server,
        item_id,
        &media_source_id,
        live_stream_id.as_deref(),
        if uses_auth_header { None } else { Some(token) },
    );

    let default_audio_stream_index = source.default_audio_stream_index;
    let default_subtitle_stream_index = source.default_subtitle_stream_index;
    let audio_stream_index = req.audio_stream_index.or(default_audio_stream_index);
    // `None` means "server default", `-1` an explicit Off — including the `-1`
    // the server itself returns when nothing is flagged.
    let play_subtitle_stream_index = req.subtitle_stream_index;
    let subtitle_stream_index = req.subtitle_stream_index.or(default_subtitle_stream_index);

    let mut external_sub_urls: Vec<(i64, String)> = maps
        .subtitle_url
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    external_sub_urls.sort_by_key(|(k, _)| *k);

    tracing::debug!(
        item = %item_id,
        embedded_subs = maps.subtitle_track_id_by_stream_index.len(),
        external_subs = external_sub_urls.len(),
        selectable_subs = maps.subtitles.len(),
        play_subtitle_stream_index = ?play_subtitle_stream_index,
        default_subtitle_stream_index,
        resolved_subtitle_stream_index = ?subtitle_stream_index,
        "prepared subtitle maps"
    );

    Ok(PreparedPlay {
        url,
        media_source_id,
        play_session_id,
        live_stream_id,
        maps,
        audio_stream_index,
        subtitle_stream_index,
        uses_auth_header,
        external_sub_urls,
        // Overwritten from `/Items/{id}` when that optional fetch succeeds.
        title: "Jellyfin".to_string(),
    })
}

/// Highest-value source, unless the caller named one: DirectPlay outweighs any
/// bitrate difference, and among equals the fattest stream wins.
pub(crate) fn select_media_source<'a>(
    sources: &'a [MediaSource],
    preferred: Option<&str>,
) -> Option<&'a MediaSource> {
    let mut selected: Option<&MediaSource> = None;
    let mut weight_selected: f64 = f64::NEG_INFINITY;
    let mut preferred_selected: Option<&MediaSource> = None;

    for source in sources {
        if let (Some(pref), Some(id)) = (preferred, source.id.as_deref())
            && id == pref
        {
            preferred_selected = Some(source);
        }
        let weight = (if source.supports_direct_play {
            50_000.0
        } else {
            0.0
        }) + source.bitrate.unwrap_or(0.0) / 1000.0;
        if selected.is_none() || weight > weight_selected {
            weight_selected = weight;
            selected = Some(source);
        }
    }

    preferred_selected.or(selected)
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
