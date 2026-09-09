//! The `PlaybackInfo` wire models, and mapping Jellyfin stream indexes to the
//! track ids mpv uses.
use super::track::TrackId;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StreamMaps {
    /// Jellyfin stream index → mpv audio track id (`aid`)
    pub(crate) audio_track_id_by_stream_index: HashMap<i64, i64>,
    /// Jellyfin stream index → mpv subtitle track id (`sid`, embedded only)
    pub(crate) subtitle_track_id_by_stream_index: HashMap<i64, i64>,
    /// Jellyfin stream index → absolute DeliveryUrl
    pub(crate) subtitle_url: HashMap<i64, String>,
    /// Every subtitle stream mpv can actually be pointed at, in listing order.
    pub(crate) subtitles: Vec<SubtitleId>,
    /// Every audio stream mpv can actually be pointed at, in listing order.
    pub(crate) audios: Vec<AudioId>,
}

/// An alias, not a distinct type — see [`TrackId`].
pub(crate) type SubtitleId = TrackId;

/// Like [`SubtitleId`], but only streams mpv has a track for: an external
/// audio stream is never loaded.
pub(crate) type AudioId = TrackId;

/// A `MediaStream`'s `Type`. Parsed rather than derived, so a value Jellyfin
/// added since cannot fail the whole response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamType {
    Audio,
    Subtitle,
    Other,
}

impl StreamType {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("Audio") => Self::Audio,
            Some("Subtitle") => Self::Subtitle,
            _ => Self::Other,
        }
    }
}

/// How Jellyfin will hand us a subtitle stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryMethod {
    /// Muxed into the file mpv is already playing.
    Embed,
    /// A separate URL to `sub-add`.
    External,
    Other,
}

impl DeliveryMethod {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("Embed") => Self::Embed,
            Some("External") => Self::External,
            _ => Self::Other,
        }
    }
}

/// One entry of a `MediaSource`'s `MediaStreams`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MediaStream {
    #[serde(rename = "Type")]
    kind: Option<String>,
    pub(crate) index: Option<i64>,
    pub(crate) is_external: bool,
    pub(crate) is_default: bool,
    pub(crate) is_forced: bool,
    pub(crate) is_external_url: bool,
    delivery_method: Option<String>,
    pub(crate) delivery_url: Option<String>,
    pub(crate) codec: Option<String>,
    pub(crate) language: Option<String>,
    /// The muxer's track name ("Signs and Songs"). More stable across episodes
    /// than `DisplayTitle`, which bakes in the language and codec.
    pub(crate) title: Option<String>,
    pub(crate) display_title: Option<String>,
    pub(crate) path: Option<String>,
}

impl MediaStream {
    pub(crate) fn kind(&self) -> StreamType {
        StreamType::parse(self.kind.as_deref())
    }

    pub(crate) fn delivery(&self) -> DeliveryMethod {
        DeliveryMethod::parse(self.delivery_method.as_deref())
    }
}

/// One `MediaSource` of a `PlaybackInfo` response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MediaSource {
    pub(crate) id: Option<String>,
    pub(crate) live_stream_id: Option<String>,
    pub(crate) supports_direct_play: bool,
    pub(crate) supports_direct_stream: bool,
    pub(crate) bitrate: Option<f64>,
    pub(crate) default_audio_stream_index: Option<i64>,
    pub(crate) default_subtitle_stream_index: Option<i64>,
    pub(crate) media_streams: Vec<MediaStream>,
}

/// A `POST /Items/{id}/PlaybackInfo` response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct PlaybackInfo {
    pub(crate) play_session_id: Option<String>,
    pub(crate) media_sources: Vec<MediaSource>,
}

pub(crate) fn map_streams(server: &str, source: &MediaSource) -> StreamMaps {
    let mut maps = StreamMaps::default();

    let mut audio_track_id = 1i64;
    for stream in &source.media_streams {
        if stream.kind() != StreamType::Audio {
            continue;
        }
        let Some(jellyfin_index) = stream.index else {
            continue;
        };
        // mpv only has a track for muxed audio; mapping an external stream
        // anyway hands back the aid of the next embedded one.
        if stream.is_external {
            tracing::debug!(
                jellyfin_index,
                "external audio stream; mpv has no track for it"
            );
            continue;
        }
        tracing::debug!(
            jellyfin_index,
            mpv_audio_track_id = audio_track_id,
            codec = stream.codec.as_deref(),
            language = stream.language.as_deref(),
            is_default = stream.is_default,
            title = stream.title.as_deref(),
            display_title = stream.display_title.as_deref(),
            "audio stream"
        );
        maps.audio_track_id_by_stream_index
            .insert(jellyfin_index, audio_track_id);
        maps.audios.push(AudioId {
            index: jellyfin_index,
            language: stream.language.clone(),
            title: stream.title.clone(),
            display_title: stream.display_title.clone(),
            codec: stream.codec.clone(),
            is_forced: stream.is_forced,
            is_external: stream.is_external,
        });
        audio_track_id += 1;
    }

    let mut subtitle_track_id = 1i64;
    for sub in &source.media_streams {
        if sub.kind() != StreamType::Subtitle {
            continue;
        }
        let Some(jellyfin_index) = sub.index else {
            continue;
        };
        tracing::debug!(
            jellyfin_index,
            delivery = ?sub.delivery(),
            codec = sub.codec.as_deref(),
            language = sub.language.as_deref(),
            is_default = sub.is_default,
            is_forced = sub.is_forced,
            is_external = sub.is_external,
            title = sub.title.as_deref(),
            display_title = sub.display_title.as_deref(),
            "subtitle stream"
        );
        // The warn arms are streams we cannot point mpv at, so they must not
        // become a remembered choice either.
        let selectable = match sub.delivery() {
            DeliveryMethod::Embed => {
                maps.subtitle_track_id_by_stream_index
                    .insert(jellyfin_index, subtitle_track_id);
                true
            }
            DeliveryMethod::External => match sub.delivery_url.as_deref() {
                Some(url) => {
                    let abs = if sub.is_external_url {
                        url.to_string()
                    } else {
                        format!("{}{url}", server.trim_end_matches('/'))
                    };
                    maps.subtitle_url.insert(jellyfin_index, abs);
                    true
                }
                None => {
                    tracing::warn!(jellyfin_index, "external subtitle has no DeliveryUrl");
                    false
                }
            },
            DeliveryMethod::Other => {
                tracing::warn!(jellyfin_index, "unmapped subtitle delivery method");
                false
            }
        };
        if selectable {
            maps.subtitles.push(SubtitleId {
                index: jellyfin_index,
                language: sub.language.clone(),
                title: sub.title.clone(),
                display_title: sub.display_title.clone(),
                codec: sub.codec.clone(),
                is_forced: sub.is_forced,
                is_external: sub.is_external,
            });
        }
        // Gated on IsExternal, not delivery: Jellyfin reports an in-file
        // subtitle as External when it extracts a sidecar, and mpv still has an
        // in-file track for it.
        if !sub.is_external {
            subtitle_track_id += 1;
        }
    }

    maps
}

/// Whether any subtitle stream comes from another origin, in which case the
/// token goes on the stream URL: mpv applies `http-header-fields` to every
/// request it makes, third-party hosts included.
pub(crate) fn has_foreign_subtitle_host(server: &str, source: &MediaSource) -> bool {
    let Ok(base) = reqwest::Url::parse(server) else {
        return false;
    };
    let mine = (base.scheme(), base.host_str(), base.port());
    source.media_streams.iter().any(|stream| {
        if stream.kind() != StreamType::Subtitle {
            return false;
        }
        let Some(path) = stream.path.as_deref() else {
            return false;
        };
        if !(path.starts_with("http://") || path.starts_with("https://")) {
            return false;
        }
        let Ok(theirs) = reqwest::Url::parse(path) else {
            return false;
        };
        theirs.host_str().is_some() && (theirs.scheme(), theirs.host_str(), theirs.port()) != mine
    })
}

pub(crate) fn mpv_audio_track_id(maps: &StreamMaps, jellyfin_index: i64) -> Option<i64> {
    maps.audio_track_id_by_stream_index
        .get(&jellyfin_index)
        .copied()
}

pub(crate) fn mpv_embedded_subtitle_track_id(
    maps: &StreamMaps,
    jellyfin_index: i64,
) -> Option<i64> {
    maps.subtitle_track_id_by_stream_index
        .get(&jellyfin_index)
        .copied()
}

/// [`mpv_embedded_subtitle_track_id`] backwards, for a track picked in the mpv
/// window. A scan: one entry per subtitle stream, read once per track change.
pub(crate) fn jellyfin_embedded_subtitle_index(
    maps: &StreamMaps,
    subtitle_track_id: i64,
) -> Option<i64> {
    maps.subtitle_track_id_by_stream_index
        .iter()
        .find(|(_, track_id)| **track_id == subtitle_track_id)
        .map(|(jellyfin_index, _)| *jellyfin_index)
}

/// [`mpv_audio_track_id`] backwards, for a track picked in the mpv window.
/// A scan: one entry per audio stream, read once per track change.
pub(crate) fn jellyfin_embedded_audio_index(maps: &StreamMaps, audio_track_id: i64) -> Option<i64> {
    maps.audio_track_id_by_stream_index
        .iter()
        .find(|(_, track_id)| **track_id == audio_track_id)
        .map(|(jellyfin_index, _)| *jellyfin_index)
}

#[cfg(test)]
#[path = "streams_test.rs"]
mod tests;
