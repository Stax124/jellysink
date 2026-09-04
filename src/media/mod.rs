//! Turning a Jellyfin item into something mpv can play.

pub(crate) mod streams;
pub(crate) mod subtitle;
pub(crate) mod title;

pub(crate) use streams::{
    MediaSource, PlaybackInfo, StreamMaps, has_foreign_subtitle_host,
    jellyfin_embedded_subtitle_index, map_streams, mpv_audio_track_id,
    mpv_embedded_subtitle_track_id,
};
pub(crate) use subtitle::{
    SubtitleMemory, SubtitlePreference, remember_subtitle_preference,
    remembered_subtitle_preference, resolve_subtitle_index,
};
pub(crate) use title::{display_title, episode_titles, item_type, series_id};

use crate::jellyfin::url::{direct_stream_url, redact_api_key};
use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;
use serde_json::Value;
use std::fmt;

/// Represents a prepared play session
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
    /// Hand-written so `url` cannot carry the access token into a log line or a
    /// color-eyre capture.
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

/// What the remote asked for when starting an item.
///
/// These four travelled as positional parameters through five layers, and three
/// of them are `Option<i64>` — so transposing two compiled fine and silently
/// picked the wrong track. Six of the eight `start_current` call sites pass
/// nothing at all, which is [`PlayRequest::default`].
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

/// Prepares a play session for the given item
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
    // `None` from Play means "use the server default". `-1` from Play is an
    // explicit Off. The server also uses `-1` when SubtitleMode=Default and
    // no stream is flagged default/forced/external — that is still a decision
    // of Off, not "unspecified".
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
        // Overwritten from `/Items/{id}` when that optional fetch succeeds; this
        // is the same fallback `display_title` uses when it does not.
        title: "Jellyfin".to_string(),
    })
}

/// Highest-value source, unless the caller named one.
///
/// DirectPlay outweighs any bitrate difference; among equals, the fattest
/// stream wins.
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
mod tests {
    use super::*;
    use serde_json::json;

    fn media_sources(v: Vec<Value>) -> Vec<MediaSource> {
        v.into_iter()
            .map(|v| MediaSource::deserialize(&v).expect("fixture should decode"))
            .collect()
    }

    fn source_direct(id: &str, bitrate: u64) -> Value {
        json!({
            "Id": id,
            "SupportsDirectPlay": true,
            "SupportsDirectStream": true,
            "SupportsTranscoding": true,
            "Bitrate": bitrate,
            "MediaStreams": []
        })
    }

    #[test]

    fn transcode_only_is_an_error() {
        let info = json!({
            "PlaySessionId": "ps",
            "MediaSources": [{
                "Id": "src",
                "SupportsDirectPlay": false,
                "SupportsDirectStream": false,
                "SupportsTranscoding": true,
                "TranscodingUrl": "/videos/x/master.m3u8",
                "MediaStreams": []
            }]
        });
        let err = prepare_play(
            "http://h:8096",
            "item",
            &info,
            &PlayRequest::default(),
            "tok",
        )
        .unwrap_err();
        assert!(err.to_string().contains("transcoding is disabled"));
    }

    #[test]

    fn prefers_direct_play_then_bitrate() {
        let sources = vec![
            json!({"Id": "low", "SupportsDirectPlay": true, "Bitrate": 1000}),
            json!({"Id": "high", "SupportsDirectPlay": true, "Bitrate": 9000000}),
            json!({"Id": "trans", "SupportsDirectPlay": false, "Bitrate": 1000}),
        ];
        let sources = media_sources(sources);
        let sel = select_media_source(&sources, None).unwrap();
        assert_eq!(sel.id.as_deref(), Some("high"));
    }

    #[test]

    fn preferred_source_wins() {
        let sources = vec![source_direct("a", 1), source_direct("b", 9)];
        let sources = media_sources(sources);
        let sel = select_media_source(&sources, Some("a")).unwrap();
        assert_eq!(sel.id.as_deref(), Some("a"));
    }

    #[test]

    fn prepare_play_happy_path() {
        let info = json!({
            "PlaySessionId": "sess",
            "MediaSources": [{
                "Id": "src",
                "SupportsDirectPlay": true,
                "SupportsDirectStream": true,
                "DefaultAudioStreamIndex": 1,
                "DefaultSubtitleStreamIndex": 2,
                "MediaStreams": [
                    {"Type": "Audio", "Index": 1, "IsExternal": false}
                ]
            }]
        });
        let prep = prepare_play(
            "http://h:8096",
            "item",
            &info,
            &PlayRequest::default(),
            "tok",
        )
        .unwrap();
        assert_eq!(prep.play_session_id, "sess");
        assert_eq!(prep.media_source_id, "src");
        assert!(prep.uses_auth_header);
        assert!(!prep.url.contains("ApiKey="));
        assert_eq!(prep.audio_stream_index, Some(1));
        assert_eq!(prep.subtitle_stream_index, Some(2));
        assert_eq!(prep.title, "Jellyfin");
    }

    #[test]

    fn prepare_play_keeps_server_default_of_off() {
        // Jellyfin SubtitleMode=Default with no default/forced/external
        // streams returns DefaultSubtitleStreamIndex=-1. That is Off, not
        // "unspecified".
        let info = json!({
            "PlaySessionId": "sess",
            "MediaSources": [{
                "Id": "src",
                "SupportsDirectPlay": true,
                "SupportsDirectStream": true,
                "DefaultAudioStreamIndex": 1,
                "DefaultSubtitleStreamIndex": -1,
                "MediaStreams": [
                    {"Type": "Audio", "Index": 1, "IsExternal": false},
                    {
                        "Type": "Subtitle",
                        "Index": 2,
                        "DeliveryMethod": "Embed",
                        "IsExternal": false,
                        "IsDefault": false,
                        "IsForced": false
                    }
                ]
            }]
        });
        let prep = prepare_play(
            "http://h:8096",
            "item",
            &info,
            &PlayRequest::default(),
            "tok",
        )
        .unwrap();
        assert_eq!(prep.subtitle_stream_index, Some(-1));
    }

    #[test]

    fn prepare_play_explicit_subtitle_stream_index_wins_over_default_off() {
        let info = json!({
            "PlaySessionId": "sess",
            "MediaSources": [{
                "Id": "src",
                "SupportsDirectPlay": true,
                "SupportsDirectStream": true,
                "DefaultSubtitleStreamIndex": -1,
                "MediaStreams": []
            }]
        });
        let prep = prepare_play(
            "http://h:8096",
            "item",
            &info,
            &PlayRequest {
                subtitle_stream_index: Some(2),
                ..Default::default()
            },
            "tok",
        )
        .unwrap();
        assert_eq!(prep.subtitle_stream_index, Some(2));
    }

    #[test]

    fn prepare_play_records_the_subtitle_identities_for_later_matching() {
        let info = json!({
            "PlaySessionId": "sess",
            "MediaSources": [{
                "Id": "src",
                "SupportsDirectPlay": true,
                "DefaultSubtitleStreamIndex": 2,
                "MediaStreams": [
                    {
                        "Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed",
                        "Language": "eng", "Title": "Signs and Songs", "IsExternal": false
                    },
                    {
                        "Type": "Subtitle", "Index": 3, "DeliveryMethod": "External",
                        "DeliveryUrl": "/Videos/i/Subtitles/3/Stream.srt",
                        "IsExternalUrl": false, "IsExternal": true,
                        "Language": "eng", "Title": "Dialogue"
                    }
                ]
            }]
        });
        let prep = prepare_play(
            "http://h:8096",
            "item",
            &info,
            &PlayRequest::default(),
            "tok",
        )
        .unwrap();
        assert_eq!(
            prep.maps
                .subtitles
                .iter()
                .map(|s| (s.index, s.title.as_deref()))
                .collect::<Vec<_>>(),
            vec![(2, Some("Signs and Songs")), (3, Some("Dialogue"))],
            "both an embedded and an external subtitle are selectable"
        );
    }

    #[test]

    fn prepared_play_debug_never_prints_the_token() {
        let prep = PreparedPlay {
            url: "http://s/Videos/i/stream?static=true&ApiKey=sekrit".into(),
            media_source_id: "m".into(),
            play_session_id: "p".into(),
            live_stream_id: None,
            maps: StreamMaps::default(),
            audio_stream_index: None,
            subtitle_stream_index: None,
            uses_auth_header: false,
            external_sub_urls: vec![],
            title: "t".into(),
        };
        let rendered = format!("{prep:?}");
        assert!(!rendered.contains("sekrit"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]

    fn a_default_play_request_is_plain_so_a_cached_prepare_can_be_reused() {
        assert!(PlayRequest::default().is_plain());
        assert!(
            PlayRequest {
                start_ticks: Some(0),
                ..Default::default()
            }
            .is_plain(),
            "a zero resume offset is still just \"play it\""
        );
    }

    #[test]

    fn any_explicit_choice_makes_a_play_request_non_plain() {
        for req in [
            PlayRequest {
                start_ticks: Some(1),
                ..Default::default()
            },
            PlayRequest {
                audio_stream_index: Some(1),
                ..Default::default()
            },
            // Some(-1) is an explicit "subtitles off", not "no preference".
            PlayRequest {
                subtitle_stream_index: Some(-1),
                ..Default::default()
            },
            PlayRequest {
                media_source_id: Some("m".into()),
                ..Default::default()
            },
        ] {
            assert!(!req.is_plain(), "{req:?} must re-fetch PlaybackInfo");
        }
    }
}
