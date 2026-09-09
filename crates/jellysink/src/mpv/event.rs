//! What an inbound mpv message means to the runtime.

use super::ipc::IpcMessage;
use serde_json::Value;

/// The mpv property holding the selected subtitle track.
pub(crate) const SUBTITLE_TRACK_PROPERTY: &str = "sid";

/// The mpv property holding the selected audio track.
pub(crate) const AUDIO_TRACK_PROPERTY: &str = "aid";

/// `observe_property` id for [`SUBTITLE_TRACK_PROPERTY`]. We match on the
/// property name, so only being distinct from other observers matters.
pub(super) const SUBTITLE_TRACK_OBSERVER_ID: i64 = 1;

/// See [`SUBTITLE_TRACK_OBSERVER_ID`]; only has to differ from it.
pub(super) const AUDIO_TRACK_OBSERVER_ID: i64 = 2;

/// What mpv answers for a track-id property such as `sid`. `false` (off) and
/// `auto` (not picked yet) must stay apart: a loading file is not a decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SelectedTrack {
    /// This track is selected.
    Id(i64),
    /// Explicitly off.
    Off,
    /// `auto`: mpv has not picked a track yet. Never a decision, and so the
    /// state every file starts and ends in.
    #[default]
    Unresolved,
}

pub(crate) fn selected_track_from_property(v: &Value) -> SelectedTrack {
    match v {
        Value::Bool(false) => SelectedTrack::Off,
        Value::String(s) if s == "no" => SelectedTrack::Off,
        // A number mpv cannot fit in an i64 is not a track id we could use.
        Value::Number(n) => n
            .as_i64()
            .map_or(SelectedTrack::Unresolved, SelectedTrack::Id),
        _ => SelectedTrack::Unresolved,
    }
}

/// Why mpv ended a file. An enum rather than a `String` so `end_file_action`
/// can match exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndFileReason {
    /// Played through to the end.
    Eof,
    /// mpv followed the file to another URL.
    Redirect,
    /// Playback was stopped — by the user, or by `playlist-next` / an OSC jump
    /// moving off the current entry.
    Stop,
    /// mpv is exiting.
    Quit,
    Error,
    /// A reason mpv added later, or no `reason` field at all.
    Other,
}

impl EndFileReason {
    fn parse(reason: Option<&str>) -> Self {
        match reason {
            Some("eof") => Self::Eof,
            Some("redirect") => Self::Redirect,
            Some("stop") => Self::Stop,
            Some("quit") => Self::Quit,
            Some("error") => Self::Error,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for EndFileReason {
    /// mpv's own spelling, so log lines read the same as mpv's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Eof => "eof",
            Self::Redirect => "redirect",
            Self::Stop => "stop",
            Self::Quit => "quit",
            Self::Error => "error",
            Self::Other => "unknown",
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum MpvEvent {
    EndFile {
        reason: EndFileReason,
    },
    FileLoaded,
    /// mpv's selected subtitle track changed — `j` in the mpv window, its track
    /// menu, or mpv auto-selecting one as a file loads.
    ///
    /// Carries no track id on purpose: these are handled a whole file load
    /// after they are emitted, so the runtime re-reads `sid` instead.
    SubtitleTrackChanged,
    /// mpv's selected audio track changed — `#` in the mpv window, its track
    /// menu, or mpv auto-selecting one as a file loads.
    ///
    /// Carries no track id, for the same reason
    /// [`MpvEvent::SubtitleTrackChanged`] does not.
    AudioTrackChanged,
    Exited,
}

/// The runtime-level event an inbound mpv message means, if any.
pub(super) fn mpv_event_for(msg: &IpcMessage) -> Option<MpvEvent> {
    match msg {
        IpcMessage::Event { name, reason } => match name.as_str() {
            "end-file" => Some(MpvEvent::EndFile {
                reason: EndFileReason::parse(reason.as_deref()),
            }),
            "file-loaded" => Some(MpvEvent::FileLoaded),
            _ => None,
        },
        IpcMessage::PropertyChange { property } => match property.as_str() {
            SUBTITLE_TRACK_PROPERTY => Some(MpvEvent::SubtitleTrackChanged),
            AUDIO_TRACK_PROPERTY => Some(MpvEvent::AudioTrackChanged),
            _ => None,
        },
        IpcMessage::Reply { .. } => None,
    }
}

#[cfg(test)]
#[path = "event_test.rs"]
mod tests;
