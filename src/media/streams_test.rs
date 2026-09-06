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

fn an_audio_track_id_maps_back_to_its_jellyfin_index() {
    let source = json!({
        "MediaStreams": [
            {"Type": "Audio", "Index": 1, "IsExternal": false},
            {"Type": "Audio", "Index": 4, "IsExternal": false},
        ]
    });
    let maps = map_streams("http://s", &media_source(source));
    assert_eq!(jellyfin_embedded_audio_index(&maps, 1), Some(1));
    assert_eq!(jellyfin_embedded_audio_index(&maps, 2), Some(4));
    // mpv numbers audio tracks from 1, so there is no third one here.
    assert_eq!(jellyfin_embedded_audio_index(&maps, 3), None);
}

#[test]

fn an_audio_identity_carries_the_raw_track_title_not_only_the_display_title() {
    let source = media_source(json!({
        "MediaStreams": [{
            "Type": "Audio", "Index": 1, "IsExternal": false,
            "Language": "jpn", "Title": "Original",
            "DisplayTitle": "Japanese - FLAC - 5.1", "Codec": "flac"
        }]
    }));
    let maps = map_streams("http://s", &source);
    assert_eq!(
        maps.audios,
        vec![AudioId {
            index: 1,
            language: Some("jpn".into()),
            title: Some("Original".into()),
            display_title: Some("Japanese - FLAC - 5.1".into()),
            codec: Some("flac".into()),
            is_forced: false,
            is_external: false,
        }]
    );
}

/// mpv never loads an external audio stream, so it can never be selected —
/// and a choice that can never be applied must not become a remembered one.
#[test]

fn an_external_audio_stream_is_not_offered_as_an_identity() {
    let source = media_source(json!({
        "MediaStreams": [
            {"Type": "Audio", "Index": 1, "IsExternal": true, "Language": "eng"},
            {"Type": "Audio", "Index": 2, "IsExternal": false, "Language": "jpn"},
        ]
    }));
    let maps = map_streams("http://s", &source);
    assert_eq!(
        maps.audios.iter().map(|a| a.index).collect::<Vec<_>>(),
        vec![2]
    );
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
