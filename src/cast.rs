use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastEvent {
    PlayNow {
        item_ids: Vec<String>,
        start_index: usize,
        start_ticks: Option<i64>,
        audio_stream_index: Option<i64>,
        subtitle_stream_index: Option<i64>,
        media_source_id: Option<String>,
    },
    PlayNext {
        item_ids: Vec<String>,
    },
    PlayLast {
        item_ids: Vec<String>,
    },
    PlayPause,
    Pause,
    Unpause,
    Stop,
    Seek {
        ticks: i64,
    },
    Next,
    Previous,
    SetVolume {
        volume: i64,
    },
    VolumeUp,
    VolumeDown,
    Mute,
    Unmute,
    ToggleMute,
    SetAudio {
        stream_index: i64,
    },
    SetSubtitle {
        stream_index: i64,
    },
    ToggleFullscreen,
}

impl CastEvent {
    pub fn from_ws(message_type: &str, data: &Value) -> Option<Self> {
        match message_type {
            "Play" => parse_play(data),
            "Playstate" => parse_playstate(data),
            "PlayPause" => Some(Self::PlayPause),
            "GeneralCommand" => parse_general(data),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    pub items: Vec<String>,
    pub index: usize,
}

impl Queue {
    pub fn current(&self) -> Option<&str> {
        self.items.get(self.index).map(String::as_str)
    }

    pub fn replace(&mut self, items: Vec<String>, start_index: usize) {
        self.items = items;
        self.index = start_index.min(self.items.len().saturating_sub(1));
        if self.items.is_empty() {
            self.index = 0;
        }
    }

    pub fn insert_next(&mut self, ids: Vec<String>) {
        let at = self.index.saturating_add(1).min(self.items.len());
        self.items.splice(at..at, ids);
    }

    pub fn append(&mut self, ids: Vec<String>) {
        self.items.extend(ids);
    }

    /// Splices `ids` in immediately before the current item, keeping `index`
    /// on the same item. Returns how many were inserted.
    ///
    /// The splice is at `index`, not at 0: mpv's playlist is the contiguous
    /// window `items[origin..origin + 1 + tail]`, so entries inserted ahead of
    /// the current item must land inside that window. Splicing at 0 would put
    /// them before `origin` and leave a hole the window arithmetic cannot see.
    pub fn insert_before_current(&mut self, ids: Vec<String>) -> usize {
        let n = ids.len();
        let at = self.index;
        self.items.splice(at..at, ids);
        self.index += n;
        n
    }

    pub fn advance(&mut self) -> Option<&str> {
        if self.index + 1 < self.items.len() {
            self.index += 1;
            self.current()
        } else {
            None
        }
    }

    pub fn previous(&mut self) -> Option<&str> {
        if self.index > 0 {
            self.index -= 1;
            self.current()
        } else {
            self.current()
        }
    }

    pub fn has_next(&self) -> bool {
        !self.items.is_empty() && self.index + 1 < self.items.len()
    }

    pub fn peek_next(&self) -> Option<&str> {
        self.items.get(self.index + 1).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndFileAction {
    Ignore,
    Advance,
    Stop,
}

pub fn end_file_action(transitioning: bool, stopping: bool, reason: &str) -> EndFileAction {
    if transitioning || stopping {
        return EndFileAction::Ignore;
    }
    match reason {
        // Advance always tries the next item (and may expand the series).
        // Stopping is play_next_or_stop's decision when nothing follows.
        "eof" | "redirect" => EndFileAction::Advance,
        "quit" | "stop" | "error" => EndFileAction::Stop,
        _ => EndFileAction::Ignore,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistEof {
    NextInMpv,
    NextNotInMpv,
    WaitForMpv,
    Stop,
}

/// `playlist-pos` p corresponds to `queue.index = origin + p`.
pub fn queue_index_at(origin: usize, playlist_pos: usize, queue_len: usize) -> Option<usize> {
    origin.checked_add(playlist_pos).filter(|i| *i < queue_len)
}

/// After EOF (caller already applied `end_file_action`). `playlist_count` is mpv's
/// playlist length, which is `Queue[origin..]` entries already appended.
///
/// `expected_pos` is the playlist index of the file that just ended
/// (`queue.index - origin`). `from_eof` is true for mpv `end-file`, false for
/// a user Next. With `keep-open=yes` mpv already auto-plays the next playlist
/// entry on EOF; `playlist-next` on top of that skips to N+2.
pub fn playlist_eof(
    playlist_pos: usize,
    playlist_count: usize,
    queue_has_next: bool,
    expected_pos: usize,
    from_eof: bool,
) -> PlaylistEof {
    if playlist_pos > expected_pos {
        PlaylistEof::WaitForMpv
    } else if playlist_pos + 1 < playlist_count {
        if from_eof {
            PlaylistEof::WaitForMpv
        } else {
            PlaylistEof::NextInMpv
        }
    } else if queue_has_next {
        PlaylistEof::NextNotInMpv
    } else {
        PlaylistEof::Stop
    }
}

/// `playlist-next` / OSC jump ends the old file with `stop`. That is not a user Stop.
pub fn ignore_stop_for_playlist(reason: &str, playlist_count: usize) -> bool {
    reason == "stop" && playlist_count > 1
}

fn parse_play(data: &Value) -> Option<CastEvent> {
    let item_ids = string_list(data.get("ItemIds")?);
    if item_ids.is_empty() {
        return None;
    }
    let start_index = data
        .get("StartIndex")
        .and_then(value_as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    let start_ticks = data.get("StartPositionTicks").and_then(value_as_i64);
    let audio_stream_index = data.get("AudioStreamIndex").and_then(value_as_i64);
    let subtitle_stream_index = data.get("SubtitleStreamIndex").and_then(value_as_i64);
    let media_source_id = data
        .get("MediaSourceId")
        .and_then(Value::as_str)
        .map(str::to_string);

    match data.get("PlayCommand").and_then(Value::as_str) {
        Some("PlayNext") => Some(CastEvent::PlayNext { item_ids }),
        Some("PlayLast") => Some(CastEvent::PlayLast { item_ids }),
        _ => Some(CastEvent::PlayNow {
            item_ids,
            start_index,
            start_ticks,
            audio_stream_index,
            subtitle_stream_index,
            media_source_id,
        }),
    }
}

fn parse_playstate(data: &Value) -> Option<CastEvent> {
    match data.get("Command").and_then(Value::as_str)? {
        "PlayPause" => Some(CastEvent::PlayPause),
        "Pause" => Some(CastEvent::Pause),
        "Unpause" => Some(CastEvent::Unpause),
        "Stop" => Some(CastEvent::Stop),
        "NextTrack" => Some(CastEvent::Next),
        "PreviousTrack" => Some(CastEvent::Previous),
        "Seek" => Some(CastEvent::Seek {
            ticks: data.get("SeekPositionTicks").and_then(value_as_i64)?,
        }),
        _ => None,
    }
}

fn parse_general(data: &Value) -> Option<CastEvent> {
    let name = data.get("Name").and_then(Value::as_str)?;
    let args = data.get("Arguments").unwrap_or(&Value::Null);
    match name {
        "SetVolume" => Some(CastEvent::SetVolume {
            volume: nested_i64(args, "Volume")?.clamp(0, 100),
        }),
        "SetAudioStreamIndex" => Some(CastEvent::SetAudio {
            stream_index: nested_i64(args, "Index")?,
        }),
        "SetSubtitleStreamIndex" => Some(CastEvent::SetSubtitle {
            stream_index: nested_i64(args, "Index")?,
        }),
        "Mute" => Some(CastEvent::Mute),
        "Unmute" => Some(CastEvent::Unmute),
        "ToggleMute" => Some(CastEvent::ToggleMute),
        "VolumeUp" => Some(CastEvent::VolumeUp),
        "VolumeDown" => Some(CastEvent::VolumeDown),
        "ToggleFullscreen" => Some(CastEvent::ToggleFullscreen),
        _ => None,
    }
}

fn string_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(arr) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn value_as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn nested_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(value_as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn play_now() {
        let ev = CastEvent::from_ws(
            "Play",
            &json!({
                "PlayCommand": "PlayNow",
                "ItemIds": ["a", "b"],
                "StartIndex": 1,
                "StartPositionTicks": 150000000,
                "AudioStreamIndex": 1,
                "SubtitleStreamIndex": 2
            }),
        )
        .unwrap();
        assert_eq!(
            ev,
            CastEvent::PlayNow {
                item_ids: vec!["a".into(), "b".into()],
                start_index: 1,
                start_ticks: Some(150_000_000),
                audio_stream_index: Some(1),
                subtitle_stream_index: Some(2),
                media_source_id: None,
            }
        );
    }

    #[test]
    fn play_without_command_is_play_now() {
        let ev = CastEvent::from_ws("Play", &json!({"ItemIds": ["x"]})).unwrap();
        assert!(matches!(ev, CastEvent::PlayNow { .. }));
    }

    #[test]
    fn play_next_and_last() {
        assert!(matches!(
            CastEvent::from_ws("Play", &json!({"PlayCommand":"PlayNext","ItemIds":["z"]})),
            Some(CastEvent::PlayNext { .. })
        ));
        assert!(matches!(
            CastEvent::from_ws("Play", &json!({"PlayCommand":"PlayLast","ItemIds":["z"]})),
            Some(CastEvent::PlayLast { .. })
        ));
    }

    #[test]
    fn playstate_seek_and_pause() {
        assert_eq!(
            CastEvent::from_ws(
                "Playstate",
                &json!({"Command":"Seek","SeekPositionTicks": 10})
            ),
            Some(CastEvent::Seek { ticks: 10 })
        );
        assert_eq!(
            CastEvent::from_ws("Playstate", &json!({"Command":"PlayPause"})),
            Some(CastEvent::PlayPause)
        );
    }

    #[test]
    fn general_volume_accepts_string() {
        let ev = CastEvent::from_ws(
            "GeneralCommand",
            &json!({"Name":"SetVolume","Arguments":{"Volume":"40"}}),
        )
        .unwrap();
        assert_eq!(ev, CastEvent::SetVolume { volume: 40 });
    }

    #[test]
    fn navigation_commands_are_ignored() {
        assert_eq!(
            CastEvent::from_ws("GeneralCommand", &json!({"Name":"MoveUp"})),
            None
        );
    }

    #[test]
    fn queue_play_next_inserts_after_current() {
        let mut q = Queue {
            items: vec!["a".into(), "c".into()],
            index: 0,
        };
        q.insert_next(vec!["b".into()]);
        assert_eq!(q.items, vec!["a", "b", "c"]);
        assert_eq!(q.advance(), Some("b"));
    }

    #[test]
    fn queue_has_next_is_false_on_the_last_item() {
        let mut q = Queue {
            items: vec!["a".into(), "b".into()],
            index: 0,
        };
        assert!(q.has_next());
        assert_eq!(q.advance(), Some("b"));
        assert!(!q.has_next());
        q.append(vec!["c".into()]);
        assert!(q.has_next());
        assert_eq!(q.current(), Some("b"));
    }

    #[test]
    fn end_file_eof_always_tries_the_next_item() {
        assert_eq!(end_file_action(false, false, "eof"), EndFileAction::Advance);
        assert_eq!(
            end_file_action(false, false, "redirect"),
            EndFileAction::Advance
        );
    }

    #[test]
    fn end_file_is_ignored_while_replacing_the_current_file() {
        assert_eq!(end_file_action(true, false, "stop"), EndFileAction::Ignore);
        assert_eq!(end_file_action(true, false, "eof"), EndFileAction::Ignore);
        assert_eq!(end_file_action(false, true, "eof"), EndFileAction::Ignore);
    }

    #[test]
    fn end_file_quit_or_error_stops() {
        assert_eq!(end_file_action(false, false, "quit"), EndFileAction::Stop);
        assert_eq!(end_file_action(false, false, "stop"), EndFileAction::Stop);
        assert_eq!(end_file_action(false, false, "error"), EndFileAction::Stop);
    }

    #[test]
    fn queue_index_tracks_playlist_origin() {
        assert_eq!(queue_index_at(2, 0, 5), Some(2));
        assert_eq!(queue_index_at(2, 2, 5), Some(4));
        assert_eq!(queue_index_at(2, 3, 5), None);
        assert_eq!(queue_index_at(0, 0, 0), None);
    }

    #[test]
    fn eof_uses_mpv_next_when_it_is_already_appended() {
        assert_eq!(playlist_eof(0, 3, true, 0, false), PlaylistEof::NextInMpv);
        assert_eq!(
            playlist_eof(2, 3, true, 2, false),
            PlaylistEof::NextNotInMpv
        );
        assert_eq!(playlist_eof(2, 3, false, 2, false), PlaylistEof::Stop);
        assert_eq!(playlist_eof(0, 1, false, 0, false), PlaylistEof::Stop);
    }

    #[test]
    fn eof_does_not_playlist_next_if_mpv_already_advanced() {
        // keep-open=yes auto-plays N+1 before we handle end-file. pos is already
        // 1 while the ended file was 0; playlist-next would skip to N+2.
        assert_eq!(playlist_eof(1, 3, true, 0, true), PlaylistEof::WaitForMpv);
        assert_eq!(playlist_eof(2, 3, true, 1, true), PlaylistEof::WaitForMpv);
    }

    #[test]
    fn eof_lets_mpv_autoplay_when_next_is_already_appended() {
        // keep-open=yes unloads the current file and starts the next one, which
        // is what emits end-file. playlist-next on top of that skips to N+2;
        // keep-open=always would pause on the last frame and never emit it.
        assert_eq!(playlist_eof(0, 3, true, 0, true), PlaylistEof::WaitForMpv);
        assert_eq!(playlist_eof(2, 3, true, 2, true), PlaylistEof::NextNotInMpv);
        assert_eq!(playlist_eof(2, 3, false, 2, true), PlaylistEof::Stop);
    }

    #[test]
    fn playlist_jump_stop_is_not_a_session_stop() {
        assert!(ignore_stop_for_playlist("stop", 3));
        assert!(!ignore_stop_for_playlist("stop", 1));
        assert!(!ignore_stop_for_playlist("eof", 3));
        assert!(!ignore_stop_for_playlist("quit", 3));
        assert!(!ignore_stop_for_playlist("error", 2));
    }

    #[test]
    fn insert_before_current_keeps_index_on_the_same_item() {
        let mut q = Queue {
            items: vec!["c".into(), "d".into()],
            index: 0,
        };
        assert_eq!(q.insert_before_current(vec!["a".into(), "b".into()]), 2);
        assert_eq!(q.items, vec!["a", "b", "c", "d"]);
        assert_eq!(q.index, 2);
        assert_eq!(q.current(), Some("c"));
    }

    #[test]
    fn insert_before_current_splices_at_the_index_not_at_zero() {
        // mpv's playlist is items[origin..origin+1+tail]; entries must land
        // inside that window, so a mid-queue insert stays put.
        let mut q = Queue {
            items: vec!["a".into(), "b".into(), "c".into()],
            index: 2,
        };
        assert_eq!(q.insert_before_current(vec!["z".into()]), 1);
        assert_eq!(q.index, 3);
        assert_eq!(q.current(), Some("c"));
        assert_eq!(q.items, vec!["a", "b", "z", "c"]);
    }

    #[test]
    fn insert_before_current_nothing_is_a_no_op() {
        let mut q = Queue {
            items: vec!["a".into()],
            index: 0,
        };
        assert_eq!(q.insert_before_current(vec![]), 0);
        assert_eq!(q.index, 0);
        assert_eq!(q.items, vec!["a"]);
    }
}
