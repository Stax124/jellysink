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
#[path = "cast_test.rs"]
mod tests;
