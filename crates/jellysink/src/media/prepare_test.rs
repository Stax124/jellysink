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
    let selected = select_media_source(&sources, None).unwrap();
    assert_eq!(selected.id.as_deref(), Some("high"));
}

#[test]
fn preferred_source_wins() {
    let sources = vec![source_direct("a", 1), source_direct("b", 9)];
    let sources = media_sources(sources);
    let selected = select_media_source(&sources, Some("a")).unwrap();
    assert_eq!(selected.id.as_deref(), Some("a"));
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
    let prepared = prepare_play(
        "http://h:8096",
        "item",
        &info,
        &PlayRequest::default(),
        "tok",
    )
    .unwrap();
    assert_eq!(prepared.play_session_id, "sess");
    assert_eq!(prepared.media_source_id, "src");
    assert!(prepared.uses_auth_header);
    assert!(!prepared.url.contains("ApiKey="));
    assert_eq!(prepared.audio_stream_index, Some(1));
    assert_eq!(prepared.subtitle_stream_index, Some(2));
    assert_eq!(prepared.title, "Jellyfin");
}

#[test]
fn prepare_play_keeps_server_default_of_off() {
    // SubtitleMode=Default with no default/forced/external streams returns
    // DefaultSubtitleStreamIndex=-1. That is Off, not "unspecified".
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
    let prepared = prepare_play(
        "http://h:8096",
        "item",
        &info,
        &PlayRequest::default(),
        "tok",
    )
    .unwrap();
    assert_eq!(prepared.subtitle_stream_index, Some(-1));
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
    let prepared = prepare_play(
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
    assert_eq!(prepared.subtitle_stream_index, Some(2));
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
    let prepared = prepare_play(
        "http://h:8096",
        "item",
        &info,
        &PlayRequest::default(),
        "tok",
    )
    .unwrap();
    assert_eq!(
        prepared
            .maps
            .subtitles
            .iter()
            .map(|s| (s.index, s.title.as_deref()))
            .collect::<Vec<_>>(),
        vec![(2, Some("Signs and Songs")), (3, Some("Dialogue"))],
        "both an embedded and an external subtitle are selectable"
    );
}

/// The MPRIS bar needs a length; a live stream has none and must not get 0.
#[test]
fn run_time_ticks_comes_from_the_chosen_source_and_only_when_positive() {
    let info = json!({
        "PlaySessionId": "ps",
        "MediaSources": [{
            "Id": "src",
            "SupportsDirectPlay": true,
            "RunTimeTicks": 14_220_809_999i64,
            "MediaStreams": []
        }]
    });
    let prepared = prepare_play(
        "http://h:8096",
        "item",
        &info,
        &PlayRequest::default(),
        "tok",
    )
    .unwrap();
    assert_eq!(prepared.run_time_ticks, Some(14_220_809_999));

    let live = json!({
        "PlaySessionId": "ps",
        "MediaSources": [{
            "Id": "src",
            "SupportsDirectPlay": true,
            "RunTimeTicks": 0,
            "MediaStreams": []
        }]
    });
    let prepared = prepare_play(
        "http://h:8096",
        "item",
        &live,
        &PlayRequest::default(),
        "tok",
    )
    .unwrap();
    assert_eq!(prepared.run_time_ticks, None);
}

#[test]
fn prepared_play_debug_never_prints_the_token() {
    let prepared = PreparedPlay {
        url: "http://s/Videos/i/stream?static=true&ApiKey=sekrit".into(),
        media_source_id: "m".into(),
        play_session_id: "p".into(),
        live_stream_id: None,
        maps: StreamMaps::default(),
        audio_stream_index: None,
        subtitle_stream_index: None,
        uses_auth_header: false,
        external_sub_urls: vec![],
        run_time_ticks: None,
        title: "t".into(),
    };
    let rendered = format!("{prepared:?}");
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
