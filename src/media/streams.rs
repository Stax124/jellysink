//! The `PlaybackInfo` wire models, and mapping Jellyfin stream indexes to the
//! track ids mpv uses.
use serde::Deserialize;
use std::collections::HashMap;

/// Maps mpv audio and subtitle track ids to Jellyfin stream indexes and vice versa.
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
}

/// One subtitle stream, identified by what it *is* rather than where it sits.
///
/// Stream indexes are per-file: the next episode can order its streams
/// differently, or come from a different provider, so "index 3" is not the same
/// track twice. Releases that split one language into `Signs and Songs` and
/// `Dialogue` also flag the wrong one as the server default often enough that
/// the index the server hands back is not trustworthy either. See
/// [`crate::media::subtitle`], which matches on this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SubtitleId {
    pub(crate) index: i64,
    pub(crate) language: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) display_title: Option<String>,
    pub(crate) codec: Option<String>,
    pub(crate) is_forced: bool,
    pub(crate) is_external: bool,
}

/// A `MediaStream`'s `Type`.
///
/// Parsed rather than derived so an unknown value cannot fail the whole
/// response; Jellyfin adds fields and values between versions.
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
    /// Anything else, or absent.
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
///
/// `#[serde(default)]` throughout: the whole surface used to be walked with
/// `get(..).and_then(Value::as_bool).unwrap_or(false)`, eight times in this
/// file alone, with `IsExternal` read twice in one loop iteration.
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
    /// Jellyfin's raw `Title` — the muxer's track name ("Signs and Songs",
    /// "Dialogue"). More stable across episodes and providers than
    /// `DisplayTitle`, which bakes in the language and the codec.
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
        // mpv only has a track for audio muxed into the file. An external audio
        // stream is never loaded, and mapping it anyway handed back the aid of
        // the *next* embedded track — selecting it played the wrong audio.
        if stream.is_external {
            tracing::debug!(
                jellyfin_index,
                "external audio stream; mpv has no track for it"
            );
            continue;
        }
        maps.audio_track_id_by_stream_index
            .insert(jellyfin_index, audio_track_id);
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
        // The two warn arms are the streams we cannot point mpv at, so they
        // also must not become a remembered choice: the user would pick one and
        // every later episode would silently fall back to the server default.
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
        // Unlike the audio loop above, this counter tracks what *mpv* sees, and
        // the Embed/External branch above tracks how *Jellyfin* delivers it.
        // The two are independent: Jellyfin reports an in-file subtitle as
        // External when it has to extract it to a sidecar, and mpv still has an
        // in-file track for it. So gate the counter on IsExternal, not on
        // DeliveryMethod, and do not skip the entry.
        if !sub.is_external {
            subtitle_track_id += 1;
        }
    }

    maps
}

/// Whether any subtitle stream is served from a different origin than the
/// Jellyfin server.
///
/// mpv applies `http-header-fields` to *every* request it makes, so if a
/// subtitle lives elsewhere the Authorization header would be sent to a third
/// party. In that case the token goes on the stream URL instead.
///
/// This built a `HashSet<String>` of hostnames whose only consumer was
/// `.is_empty()`, allocating for every subtitle stream on every prepare.
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

/// Resolve a Jellyfin audio stream index to an mpv audio track id, if mapped.
pub(crate) fn mpv_audio_track_id(maps: &StreamMaps, jellyfin_index: i64) -> Option<i64> {
    maps.audio_track_id_by_stream_index
        .get(&jellyfin_index)
        .copied()
}

/// Resolve a Jellyfin subtitle stream index to an embedded mpv subtitle track id.
pub(crate) fn mpv_embedded_subtitle_track_id(
    maps: &StreamMaps,
    jellyfin_index: i64,
) -> Option<i64> {
    maps.subtitle_track_id_by_stream_index
        .get(&jellyfin_index)
        .copied()
}

/// Resolve an embedded mpv subtitle track id (`sid`) back to its Jellyfin
/// stream index — [`mpv_embedded_subtitle_track_id`] backwards, for a track the
/// user picked in the mpv window rather than in a Jellyfin client.
///
/// A linear scan of a map that holds one entry per subtitle stream in the file;
/// keeping a second `HashMap` in sync for a handful of entries read once per
/// track change is not worth it.
pub(crate) fn jellyfin_embedded_subtitle_index(
    maps: &StreamMaps,
    subtitle_track_id: i64,
) -> Option<i64> {
    maps.subtitle_track_id_by_stream_index
        .iter()
        .find(|(_, track_id)| **track_id == subtitle_track_id)
        .map(|(jellyfin_index, _)| *jellyfin_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// The json! fixtures below are the server's actual wire shape; these turn
    /// them into the typed models the code now works with.
    fn media_source(v: Value) -> MediaSource {
        MediaSource::deserialize(&v).expect("fixture should decode")
    }

    #[test]

    fn maps_embedded_and_external_subs() {
        let source = json!({
            "MediaStreams": [
                {"Type": "Audio", "Index": 1, "IsExternal": false},
                {"Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed", "IsExternal": false},
                {
                    "Type": "Subtitle",
                    "Index": 3,
                    "DeliveryMethod": "External",
                    "DeliveryUrl": "/Videos/i/Subtitles/3/Stream.srt",
                    "IsExternalUrl": false,
                    "IsExternal": true
                }
            ]
        });
        let maps = map_streams("http://h:8096", &media_source(source));
        assert_eq!(maps.audio_track_id_by_stream_index.get(&1), Some(&1));
        assert_eq!(maps.subtitle_track_id_by_stream_index.get(&2), Some(&1));
        assert_eq!(
            maps.subtitle_url.get(&3).map(String::as_str),
            Some("http://h:8096/Videos/i/Subtitles/3/Stream.srt")
        );
    }

    #[test]

    fn an_embedded_subtitle_track_id_maps_back_to_its_jellyfin_index() {
        let source = json!({
            "MediaStreams": [
                {"Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed", "IsExternal": false},
                {"Type": "Subtitle", "Index": 5, "DeliveryMethod": "Embed", "IsExternal": false},
            ]
        });
        let maps = map_streams("http://s", &media_source(source));
        assert_eq!(jellyfin_embedded_subtitle_index(&maps, 1), Some(2));
        assert_eq!(jellyfin_embedded_subtitle_index(&maps, 2), Some(5));
        // A `sub-add`ed external track; the runtime resolves those from its own
        // map, and this one must not guess at an embedded stream.
        assert_eq!(jellyfin_embedded_subtitle_index(&maps, 3), None);
    }

    #[test]

    fn an_external_audio_stream_does_not_steal_the_next_embedded_track_id() {
        let source = json!({
            "MediaStreams": [
                {"Type": "Audio", "Index": 1, "IsExternal": true},
                {"Type": "Audio", "Index": 2, "IsExternal": false},
            ]
        });
        let maps = map_streams("http://s", &media_source(source));
        // mpv never loads the external stream, so it has no aid at all.
        assert_eq!(mpv_audio_track_id(&maps, 1), None);
        // The embedded stream is mpv's first audio track, not its second.
        assert_eq!(mpv_audio_track_id(&maps, 2), Some(1));
    }

    #[test]

    fn embedded_audio_tracks_are_numbered_from_one_in_order() {
        let source = json!({
            "MediaStreams": [
                {"Type": "Audio", "Index": 1, "IsExternal": false},
                {"Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed", "IsExternal": false},
                {"Type": "Audio", "Index": 3, "IsExternal": false},
            ]
        });
        let maps = map_streams("http://s", &media_source(source));
        assert_eq!(mpv_audio_track_id(&maps, 1), Some(1));
        assert_eq!(mpv_audio_track_id(&maps, 3), Some(2));
    }

    /// An in-file subtitle that Jellyfin delivers as a sidecar still occupies an    /// mpv track, so the subtitle counter must keep counting it.

    #[test]

    fn an_extracted_subtitle_still_advances_the_mpv_subtitle_numbering() {
        let source = json!({
            "MediaStreams": [
                {
                    "Type": "Subtitle", "Index": 1, "DeliveryMethod": "External",
                    "DeliveryUrl": "/sub1.srt", "IsExternalUrl": false, "IsExternal": false
                },
                {"Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed", "IsExternal": false},
            ]
        });
        let maps = map_streams("http://s", &media_source(source));
        assert_eq!(mpv_embedded_subtitle_track_id(&maps, 2), Some(2));
    }

    #[test]

    fn a_subtitle_identity_carries_the_raw_track_title_not_only_the_display_title() {
        let source = media_source(json!({
            "MediaStreams": [{
                "Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed",
                "Language": "eng", "Title": "Dialogue",
                "DisplayTitle": "English - Dialogue - SRT", "Codec": "subrip",
                "IsForced": false, "IsExternal": false
            }]
        }));
        let maps = map_streams("http://s", &source);
        assert_eq!(
            maps.subtitles,
            vec![SubtitleId {
                index: 2,
                language: Some("eng".into()),
                title: Some("Dialogue".into()),
                display_title: Some("English - Dialogue - SRT".into()),
                codec: Some("subrip".into()),
                is_forced: false,
                is_external: false,
            }]
        );
    }

    /// A stream we cannot point mpv at must not become a remembered choice —
    /// the user would pick it once and every later episode would silently fall
    /// back to the server default.
    #[test]

    fn an_unselectable_subtitle_is_not_offered_as_an_identity() {
        let source = media_source(json!({
            "MediaStreams": [
                {"Type": "Subtitle", "Index": 1, "DeliveryMethod": "Hls", "Language": "eng"},
                {"Type": "Subtitle", "Index": 2, "DeliveryMethod": "External", "Language": "eng"},
                {"Type": "Subtitle", "Index": 3, "DeliveryMethod": "Embed", "Language": "eng"},
            ]
        }));
        let maps = map_streams("http://s", &source);
        assert_eq!(
            maps.subtitles.iter().map(|s| s.index).collect::<Vec<_>>(),
            vec![3],
            "Hls has no mapping and the External entry has no DeliveryUrl"
        );
    }

    #[test]

    fn an_extracted_subtitle_is_still_a_selectable_identity() {
        let source = media_source(json!({
            "MediaStreams": [{
                "Type": "Subtitle", "Index": 1, "DeliveryMethod": "External",
                "DeliveryUrl": "/sub1.srt", "IsExternalUrl": false, "IsExternal": false,
                "Language": "ces"
            }]
        }));
        let maps = map_streams("http://s", &source);
        assert_eq!(
            maps.subtitles.iter().map(|s| s.index).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]

    fn an_unknown_delivery_method_does_not_fail_the_whole_response() {
        // Jellyfin adds values between versions; a strict enum would refuse the
        // entire PlaybackInfo over one unrecognised subtitle.
        let source = media_source(json!({
            "MediaStreams": [
                {"Type": "Subtitle", "Index": 1, "DeliveryMethod": "Hls"},
                {"Type": "Subtitle", "Index": 2, "DeliveryMethod": "Embed"},
            ]
        }));
        assert_eq!(source.media_streams[0].delivery(), DeliveryMethod::Other);
        let maps = map_streams("http://s", &source);
        assert_eq!(mpv_embedded_subtitle_track_id(&maps, 2), Some(2));
    }

    #[test]

    fn an_unknown_stream_type_is_neither_audio_nor_subtitle() {
        let source = media_source(json!({
            "MediaStreams": [{"Type": "Video", "Index": 0}]
        }));
        assert_eq!(source.media_streams[0].kind(), StreamType::Other);
        let maps = map_streams("http://s", &source);
        assert!(maps.audio_track_id_by_stream_index.is_empty());
        assert!(maps.subtitle_track_id_by_stream_index.is_empty());
    }

    #[test]

    fn missing_fields_default_rather_than_failing() {
        let source = media_source(json!({}));
        assert!(!source.supports_direct_play);
        assert_eq!(source.id, None);
        assert!(source.media_streams.is_empty());
    }

    #[test]

    fn a_subtitle_on_the_jellyfin_host_is_not_foreign() {
        let source = media_source(json!({
            "MediaStreams": [
                {"Type": "Subtitle", "Index": 1, "Path": "http://h:8096/subs/1.srt"},
            ]
        }));
        assert!(!has_foreign_subtitle_host("http://h:8096", &source));
    }

    /// mpv sends `http-header-fields` to every request it makes, so a subtitle    /// on a third-party host would receive the Authorization header.

    #[test]

    fn a_subtitle_on_another_host_is_foreign() {
        for path in [
            "http://elsewhere/1.srt",
            "https://h:8096/1.srt",
            "http://h:9000/1.srt",
        ] {
            let source = media_source(json!({
                "MediaStreams": [{"Type": "Subtitle", "Index": 1, "Path": path}],
            }));
            assert!(
                has_foreign_subtitle_host("http://h:8096", &source),
                "{path} should be foreign"
            );
        }
    }

    #[test]

    fn a_local_subtitle_path_is_not_a_foreign_host() {
        let source = media_source(json!({
            "MediaStreams": [{"Type": "Subtitle", "Index": 1, "Path": "/media/show/1.srt"}],
        }));
        assert!(!has_foreign_subtitle_host("http://h:8096", &source));
    }
}
