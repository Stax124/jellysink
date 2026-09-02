use color_eyre::eyre::eyre;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Maps mpv audio and subtitle track ids to Jellyfin stream indexes and vice versa.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamMaps {
    /// mpv audio track id (`aid`) → Jellyfin stream index
    pub audio_stream_index_by_track_id: HashMap<i64, i64>,
    /// Jellyfin stream index → mpv audio track id (`aid`)
    pub audio_track_id_by_stream_index: HashMap<i64, i64>,
    /// mpv subtitle track id (`sid`, embedded only) → Jellyfin stream index
    pub subtitle_stream_index_by_track_id: HashMap<i64, i64>,
    /// Jellyfin stream index → mpv subtitle track id (`sid`, embedded only)
    pub subtitle_track_id_by_stream_index: HashMap<i64, i64>,
    /// Jellyfin stream index → absolute DeliveryUrl
    pub subtitle_url: HashMap<i64, String>,
}

/// Represents a prepared play session
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPlay {
    pub url: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub live_stream_id: Option<String>,
    pub maps: StreamMaps,
    pub audio_stream_index: Option<i64>,
    pub subtitle_stream_index: Option<i64>,
    pub uses_auth_header: bool,
    pub external_sub_urls: Vec<(i64, String)>,
    pub title: String,
}

/// Prepares a play session for the given item
pub fn prepare_play(
    server: &str,
    item_id: &str,
    playback_info: &Value,
    preferred_source: Option<&str>,
    audio_stream_index: Option<i64>,
    subtitle_stream_index: Option<i64>,
    token: &str,
) -> color_eyre::Result<PreparedPlay> {
    let play_session_id = playback_info
        .get("PlaySessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| eyre!("PlaybackInfo missing PlaySessionId"))?
        .to_string();

    let sources = playback_info
        .get("MediaSources")
        .and_then(Value::as_array)
        .ok_or_else(|| eyre!("PlaybackInfo missing MediaSources"))?;

    let source = select_media_source(sources, preferred_source)
        .ok_or_else(|| eyre!("PlaybackInfo has no media sources"))?;

    let supports_direct_play = source
        .get("SupportsDirectPlay")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let supports_direct_stream = source
        .get("SupportsDirectStream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if !supports_direct_play && !supports_direct_stream {
        return Err(eyre!(
            "server will not DirectPlay this item (SupportsDirectPlay=false, SupportsDirectStream=false); transcoding is disabled"
        ));
    }

    let media_source_id = source
        .get("Id")
        .and_then(Value::as_str)
        .ok_or_else(|| eyre!("MediaSource missing Id"))?
        .to_string();

    let live_stream_id = source
        .get("LiveStreamId")
        .and_then(Value::as_str)
        .map(str::to_string);

    let maps = map_streams(server, source);
    let foreign = foreign_subtitle_hosts(server, source);
    let uses_auth_header = foreign.is_empty();

    let url = direct_stream_url(
        server,
        item_id,
        &media_source_id,
        live_stream_id.as_deref(),
        if uses_auth_header { None } else { Some(token) },
    );

    let default_audio_stream_index = source
        .get("DefaultAudioStreamIndex")
        .and_then(Value::as_i64);
    let default_subtitle_stream_index = source
        .get("DefaultSubtitleStreamIndex")
        .and_then(Value::as_i64);
    let audio_stream_index = audio_stream_index.or(default_audio_stream_index);
    // `None` from Play means "use the server default". `-1` from Play is an
    // explicit Off. The server also uses `-1` when SubtitleMode=Default and
    // no stream is flagged default/forced/external — that is still a decision
    // of Off, not "unspecified".
    let play_subtitle_stream_index = subtitle_stream_index;
    let subtitle_stream_index = subtitle_stream_index.or(default_subtitle_stream_index);

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
        title: "Jellyfin".to_string(),
    })
}

/// Window / OSC title, matching jellyfin-mpv-shim's `get_proper_title`.
pub fn display_title(item: &Value) -> String {
    let name = item
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or("Jellyfin");
    let is_episode = item.get("Type").and_then(Value::as_str) == Some("Episode");
    if is_episode
        && let (Some(series), Some(season), Some(episode)) = (
            item.get("SeriesName").and_then(Value::as_str),
            item.get("ParentIndexNumber").and_then(Value::as_i64),
            item.get("IndexNumber").and_then(Value::as_i64),
        )
    {
        return format!("{series} - s{season}e{episode:02} - {name}");
    }
    if item.get("Type").and_then(Value::as_str) == Some("Movie")
        && let Some(year) = item.get("ProductionYear").and_then(Value::as_i64)
    {
        return format!("{name} ({year})");
    }
    name.to_string()
}

pub fn select_media_source<'a>(sources: &'a [Value], preferred: Option<&str>) -> Option<&'a Value> {
    let mut selected: Option<&Value> = None;
    let mut weight_selected: f64 = f64::NEG_INFINITY;
    let mut preferred_selected: Option<&Value> = None;

    for source in sources {
        if let (Some(pref), Some(id)) = (preferred, source.get("Id").and_then(Value::as_str))
            && id == pref
        {
            preferred_selected = Some(source);
        }
        let direct = source
            .get("SupportsDirectPlay")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let bitrate = source.get("Bitrate").and_then(Value::as_f64).unwrap_or(0.0);
        let weight = (if direct { 50_000.0 } else { 0.0 }) + bitrate / 1000.0;
        if selected.is_none() || weight > weight_selected {
            weight_selected = weight;
            selected = Some(source);
        }
    }

    preferred_selected.or(selected)
}

pub fn direct_stream_url(
    server: &str,
    item_id: &str,
    media_source_id: &str,
    live_stream_id: Option<&str>,
    token: Option<&str>,
) -> String {
    let server = server.trim_end_matches('/');
    let mut url =
        format!("{server}/Videos/{item_id}/stream?static=true&MediaSourceId={media_source_id}");
    if let Some(live) = live_stream_id {
        url.push_str("&LiveStreamId=");
        url.push_str(live);
    }
    if let Some(token) = token {
        url.push_str("&ApiKey=");
        url.push_str(token);
    }
    url
}

pub fn map_streams(server: &str, source: &Value) -> StreamMaps {
    let mut maps = StreamMaps::default();
    let Some(streams) = source.get("MediaStreams").and_then(Value::as_array) else {
        return maps;
    };

    let mut audio_track_id = 1i64;
    for stream in streams {
        if stream.get("Type").and_then(Value::as_str) != Some("Audio") {
            continue;
        }
        let Some(jellyfin_index) = stream.get("Index").and_then(Value::as_i64) else {
            continue;
        };
        maps.audio_stream_index_by_track_id
            .insert(audio_track_id, jellyfin_index);
        maps.audio_track_id_by_stream_index
            .insert(jellyfin_index, audio_track_id);
        if !stream
            .get("IsExternal")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            audio_track_id += 1;
        }
    }

    let mut subtitle_track_id = 1i64;
    for sub in streams {
        if sub.get("Type").and_then(Value::as_str) != Some("Subtitle") {
            continue;
        }
        let Some(jellyfin_index) = sub.get("Index").and_then(Value::as_i64) else {
            continue;
        };
        let delivery = sub.get("DeliveryMethod").and_then(Value::as_str);
        let codec = sub.get("Codec").and_then(Value::as_str);
        let is_external = sub
            .get("IsExternal")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let is_default = sub
            .get("IsDefault")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let is_forced = sub
            .get("IsForced")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let language = sub.get("Language").and_then(Value::as_str);
        let title = sub.get("DisplayTitle").and_then(Value::as_str);
        tracing::debug!(
            jellyfin_index,
            delivery,
            codec,
            language,
            is_default,
            is_forced,
            is_external,
            title,
            "subtitle stream"
        );
        match delivery {
            Some("Embed") => {
                maps.subtitle_stream_index_by_track_id
                    .insert(subtitle_track_id, jellyfin_index);
                maps.subtitle_track_id_by_stream_index
                    .insert(jellyfin_index, subtitle_track_id);
            }
            Some("External") => {
                if let Some(url) = sub.get("DeliveryUrl").and_then(Value::as_str) {
                    let abs = if sub
                        .get("IsExternalUrl")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        url.to_string()
                    } else {
                        format!("{}{url}", server.trim_end_matches('/'))
                    };
                    maps.subtitle_url.insert(jellyfin_index, abs);
                } else {
                    tracing::warn!(jellyfin_index, "external subtitle has no DeliveryUrl");
                }
            }
            Some(other) => {
                tracing::warn!(
                    jellyfin_index,
                    method = other,
                    "unmapped subtitle delivery method"
                );
            }
            None => {
                tracing::warn!(jellyfin_index, "subtitle stream has no DeliveryMethod");
            }
        }
        if !sub
            .get("IsExternal")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            subtitle_track_id += 1;
        }
    }

    maps
}

pub fn foreign_subtitle_hosts(server: &str, source: &Value) -> HashSet<String> {
    let mut foreign = HashSet::new();
    let Ok(base) = reqwest::Url::parse(server) else {
        return foreign;
    };
    let mine = (
        base.scheme().to_string(),
        base.host_str().map(str::to_string),
        base.port(),
    );
    let Some(streams) = source.get("MediaStreams").and_then(Value::as_array) else {
        return foreign;
    };
    for stream in streams {
        if stream.get("Type").and_then(Value::as_str) != Some("Subtitle") {
            continue;
        }
        let Some(path) = stream.get("Path").and_then(Value::as_str) else {
            continue;
        };
        if !(path.starts_with("http://") || path.starts_with("https://")) {
            continue;
        }
        let Ok(parts) = reqwest::Url::parse(path) else {
            continue;
        };
        let theirs = (
            parts.scheme().to_string(),
            parts.host_str().map(str::to_string),
            parts.port(),
        );
        if theirs != mine
            && let Some(h) = parts.host_str()
        {
            foreign.insert(h.to_string());
        }
    }
    foreign
}

pub fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / 10_000_000.0
}

pub fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds * 10_000_000.0).round() as i64
}

/// Prefer a live mpv sample, but never replace a known position with 0/missing.
/// A Stopped POST of 0 wipes Jellyfin's resume point (`UpdatePlayState`).
pub fn coalesce_position_ticks(live_seconds: Option<f64>, last_ticks: i64) -> i64 {
    live_seconds
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(seconds_to_ticks)
        .unwrap_or(last_ticks)
}

/// Resolve a Jellyfin audio stream index to an mpv audio track id, if mapped.
pub fn mpv_audio_track_id(maps: &StreamMaps, jellyfin_index: i64) -> Option<i64> {
    maps.audio_track_id_by_stream_index
        .get(&jellyfin_index)
        .copied()
}

/// Resolve a Jellyfin subtitle stream index to an embedded mpv subtitle track id.
pub fn mpv_embedded_subtitle_track_id(maps: &StreamMaps, jellyfin_index: i64) -> Option<i64> {
    maps.subtitle_track_id_by_stream_index
        .get(&jellyfin_index)
        .copied()
}

/// Splits a full series listing into the ids before and after `current_id`.
///
/// Returns `(previous, remaining)`. Empty on both sides when `current_id` is
/// not in the listing — fail closed on specials / library churn.
pub fn split_episode_ids(episodes: &Value, current_id: &str) -> (Vec<String>, Vec<String>) {
    let Some(items) = episodes.get("Items").and_then(Value::as_array) else {
        return (Vec::new(), Vec::new());
    };
    let ids: Vec<String> = items
        .iter()
        .filter_map(|it| it.get("Id").and_then(Value::as_str).map(str::to_string))
        .collect();
    match ids.iter().position(|id| id == current_id) {
        Some(i) => {
            let (before, after) = ids.split_at(i);
            (before.to_vec(), after[1..].to_vec())
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// Display titles from a series episode listing, keyed by item id.
///
/// Playlist fill uses this so mpv's selector has names without a per-item
/// `PlaybackInfo` / `GET /Items/{id}` round trip.
pub fn episode_titles(listing: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(items) = listing.get("Items").and_then(Value::as_array) else {
        return out;
    };
    for it in items {
        let Some(id) = it.get("Id").and_then(Value::as_str) else {
            continue;
        };
        out.insert(id.to_string(), display_title(it));
    }
    out
}

pub fn item_type(item: &Value) -> Option<&str> {
    item.get("Type").and_then(Value::as_str)
}

pub fn series_id(item: &Value) -> Option<&str> {
    item.get("SeriesId").and_then(Value::as_str)
}

/// Whether this item could have previous episodes worth prepending.
///
/// Deliberately ignores `has_next`, unlike [`series_expand_skip_reason`]:
/// Jellyfin sending 6..20 is exactly when we also want 1..5. Also ignores
/// `autoplay`, which governs continuing *forward*, not what the playlist
/// selector can reach.
pub fn prepend_skip_reason(
    item_type: Option<&str>,
    series_id: Option<&str>,
    prepend_previous: bool,
) -> Option<&'static str> {
    if !prepend_previous {
        return Some("prepend_previous disabled");
    }
    if item_type != Some("Episode") {
        return Some("item is not an episode");
    }
    if series_id.is_none() {
        return Some("item has no SeriesId");
    }
    None
}

/// The subset of `ids` not already present in `queue`.
///
/// Keeps prepending idempotent: advancing e6 -> e7 leaves e1..e6 already in
/// the queue ahead of e7, so re-running the expand must not add them twice.
pub fn ids_missing_from(ids: &[String], queue: &[String]) -> Vec<String> {
    let present: HashSet<&str> = queue.iter().map(String::as_str).collect();
    ids.iter()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect()
}

pub fn series_expand_skip_reason(
    item_type: Option<&str>,
    series_id: Option<&str>,
    has_next: bool,
    autoplay: bool,
) -> Option<&'static str> {
    if !autoplay {
        return Some("autoplay disabled");
    }
    if has_next {
        return Some("queue already has a next item");
    }
    if item_type != Some("Episode") {
        return Some("item is not an episode");
    }
    if series_id.is_none() {
        return Some("item has no SeriesId");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn url_direct_stream_without_token() {
        let url = direct_stream_url("http://h:8096", "item1", "src1", None, None);
        assert_eq!(
            url,
            "http://h:8096/Videos/item1/stream?static=true&MediaSourceId=src1"
        );
    }

    #[test]
    fn url_puts_apikey_when_no_header() {
        let url = direct_stream_url("http://h:8096", "item1", "src1", None, Some("tok"));
        assert!(url.contains("ApiKey=tok"));
        assert!(url.contains("static=true"));
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
        let err =
            prepare_play("http://h:8096", "item", &info, None, None, None, "tok").unwrap_err();
        assert!(err.to_string().contains("transcoding is disabled"));
    }

    #[test]
    fn prefers_direct_play_then_bitrate() {
        let sources = vec![
            json!({"Id": "low", "SupportsDirectPlay": true, "Bitrate": 1000}),
            json!({"Id": "high", "SupportsDirectPlay": true, "Bitrate": 9000000}),
            json!({"Id": "trans", "SupportsDirectPlay": false, "Bitrate": 1000}),
        ];
        let sel = select_media_source(&sources, None).unwrap();
        assert_eq!(sel["Id"], "high");
    }

    #[test]
    fn preferred_source_wins() {
        let sources = vec![source_direct("a", 1), source_direct("b", 9)];
        let sel = select_media_source(&sources, Some("a")).unwrap();
        assert_eq!(sel["Id"], "a");
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
        let maps = map_streams("http://h:8096", &source);
        assert_eq!(maps.audio_track_id_by_stream_index.get(&1), Some(&1));
        assert_eq!(maps.subtitle_track_id_by_stream_index.get(&2), Some(&1));
        assert_eq!(
            maps.subtitle_url.get(&3).map(String::as_str),
            Some("http://h:8096/Videos/i/Subtitles/3/Stream.srt")
        );
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
        let prep = prepare_play("http://h:8096", "item", &info, None, None, None, "tok").unwrap();
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
        let prep = prepare_play("http://h:8096", "item", &info, None, None, None, "tok").unwrap();
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
        let prep =
            prepare_play("http://h:8096", "item", &info, None, None, Some(2), "tok").unwrap();
        assert_eq!(prep.subtitle_stream_index, Some(2));
    }

    #[test]
    fn episode_title_includes_series_and_numbers() {
        let item = json!({
            "Type": "Episode",
            "Name": "The One",
            "SeriesName": "Friends",
            "ParentIndexNumber": 1,
            "IndexNumber": 2
        });
        assert_eq!(display_title(&item), "Friends - s1e02 - The One");
    }

    #[test]
    fn movie_title_includes_year() {
        let item = json!({"Type": "Movie", "Name": "Heat", "ProductionYear": 1995});
        assert_eq!(display_title(&item), "Heat (1995)");
    }

    #[test]
    fn plain_name_when_metadata_is_thin() {
        let item = json!({"Name": "Home Video"});
        assert_eq!(display_title(&item), "Home Video");
    }

    #[test]
    fn ticks_roundtrip() {
        assert!((ticks_to_seconds(150_000_000) - 15.0).abs() < f64::EPSILON);
        assert_eq!(seconds_to_ticks(15.0), 150_000_000);
    }

    #[test]
    fn coalesce_uses_a_live_position() {
        assert_eq!(coalesce_position_ticks(Some(42.0), 0), 420_000_000);
    }

    #[test]
    fn coalesce_keeps_last_ticks_when_live_is_missing() {
        assert_eq!(coalesce_position_ticks(None, 150_000_000), 150_000_000);
    }

    #[test]
    fn coalesce_does_not_regress_to_zero_on_a_dead_sample() {
        // Closing mpv makes time-pos fail or return 0; a Stopped POST of 0
        // wipes the resume point Jellyfin already stored from progress ticks.
        assert_eq!(coalesce_position_ticks(Some(0.0), 150_000_000), 150_000_000);
        assert_eq!(
            coalesce_position_ticks(Some(f64::NAN), 150_000_000),
            150_000_000
        );
    }

    #[test]
    fn coalesce_zero_when_nothing_has_played() {
        assert_eq!(coalesce_position_ticks(None, 0), 0);
        assert_eq!(coalesce_position_ticks(Some(0.0), 0), 0);
    }

    #[test]
    fn coalesce_eof_reports_the_live_end() {
        assert_eq!(
            coalesce_position_ticks(Some(100.0), 900_000_000),
            1_000_000_000
        );
    }

    fn episodes_json(ids: &[&str]) -> Value {
        json!({
            "Items": ids.iter().map(|id| json!({"Id": id})).collect::<Vec<_>>()
        })
    }

    #[test]
    fn split_episodes_returns_previous_and_remaining() {
        let v = episodes_json(&["e1", "e2", "e3", "e4"]);
        let (previous, remaining) = split_episode_ids(&v, "e3");
        assert_eq!(previous, vec!["e1".to_string(), "e2".to_string()]);
        assert_eq!(remaining, vec!["e4".to_string()]);
    }

    #[test]
    fn split_episodes_at_the_first_has_no_previous() {
        let v = episodes_json(&["e1", "e2"]);
        let (previous, remaining) = split_episode_ids(&v, "e1");
        assert!(previous.is_empty());
        assert_eq!(remaining, vec!["e2".to_string()]);
    }

    #[test]
    fn split_episodes_at_the_last_has_no_remaining() {
        let v = episodes_json(&["e1", "e2"]);
        let (previous, remaining) = split_episode_ids(&v, "e2");
        assert_eq!(previous, vec!["e1".to_string()]);
        assert!(remaining.is_empty());
    }

    #[test]
    fn split_episodes_empty_when_current_is_missing() {
        let v = episodes_json(&["e1", "e2"]);
        assert_eq!(split_episode_ids(&v, "special"), (vec![], vec![]));
    }

    #[test]
    fn split_episodes_empty_on_malformed_payload() {
        assert_eq!(split_episode_ids(&json!({}), "e1"), (vec![], vec![]));
        assert_eq!(
            split_episode_ids(&json!({"Items": "nope"}), "e1"),
            (vec![], vec![])
        );
    }

    #[test]
    fn episode_titles_use_display_title() {
        let v = json!({
            "Items": [
                {
                    "Id": "e1",
                    "Type": "Episode",
                    "Name": "Pilot",
                    "SeriesName": "Show",
                    "ParentIndexNumber": 1,
                    "IndexNumber": 1
                },
                {
                    "Id": "e2",
                    "Type": "Episode",
                    "Name": "Next",
                    "SeriesName": "Show",
                    "ParentIndexNumber": 1,
                    "IndexNumber": 2
                }
            ]
        });
        let titles = episode_titles(&v);
        assert_eq!(titles.get("e1").unwrap(), "Show - s1e01 - Pilot");
        assert_eq!(titles.get("e2").unwrap(), "Show - s1e02 - Next");
    }

    #[test]
    fn episode_titles_empty_on_malformed_payload() {
        assert!(episode_titles(&json!({})).is_empty());
        assert!(episode_titles(&json!({"Items": "nope"})).is_empty());
    }

    #[test]
    fn prepend_runs_when_the_queue_already_has_a_next_item() {
        // The bug: Jellyfin sends 6..20, so has_next is true and the forward
        // gate bails. Prepending must not share that gate.
        assert_eq!(
            prepend_skip_reason(Some("Episode"), Some("series-1"), true),
            None
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), true, true),
            Some("queue already has a next item")
        );
    }

    #[test]
    fn prepend_ignores_autoplay() {
        // autoplay governs continuing forward, not what the selector reaches.
        assert_eq!(
            prepend_skip_reason(Some("Episode"), Some("series-1"), true),
            None
        );
    }

    #[test]
    fn prepend_respects_its_own_toggle() {
        assert_eq!(
            prepend_skip_reason(Some("Episode"), Some("series-1"), false),
            Some("prepend_previous disabled")
        );
    }

    #[test]
    fn prepend_skips_non_episodes_and_seriesless_items() {
        assert_eq!(
            prepend_skip_reason(Some("Movie"), Some("series-1"), true),
            Some("item is not an episode")
        );
        assert_eq!(
            prepend_skip_reason(Some("Episode"), None, true),
            Some("item has no SeriesId")
        );
    }

    #[test]
    fn ids_missing_from_drops_what_the_queue_already_has() {
        let previous = ["e1".to_string(), "e2".to_string(), "e3".to_string()];
        let queue = ["e1".to_string(), "e2".to_string(), "e4".to_string()];
        assert_eq!(ids_missing_from(&previous, &queue), vec!["e3".to_string()]);
    }

    #[test]
    fn ids_missing_from_is_empty_when_all_present() {
        let previous = ["e1".to_string(), "e2".to_string()];
        let queue = ["e1".to_string(), "e2".to_string(), "e3".to_string()];
        assert!(ids_missing_from(&previous, &queue).is_empty());
    }

    #[test]
    fn ids_missing_from_keeps_order() {
        let previous = ["e3".to_string(), "e1".to_string(), "e2".to_string()];
        assert_eq!(
            ids_missing_from(&previous, &[]),
            vec!["e3".to_string(), "e1".to_string(), "e2".to_string()]
        );
    }

    #[test]
    fn expand_series_only_for_a_lonely_episode() {
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), false, true),
            None
        );
        assert_eq!(
            series_expand_skip_reason(Some("Movie"), Some("series-1"), false, true),
            Some("item is not an episode")
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), None, false, true),
            Some("item has no SeriesId")
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), true, true),
            Some("queue already has a next item")
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), false, false),
            Some("autoplay disabled")
        );
    }
}
