//! What an inbound mpv message means to the runtime.

use crate::media::TrackKind;
use serde_json::Value;

/// What mpv answers for a track-id property such as `sid`. `false` (off) and
/// `auto` (not picked yet) must stay apart: a loading file is not a decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SelectedTrack {
    Id(i64),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MpvEvent {
    EndFile {
        reason: EndFileReason,
    },
    FileLoaded,
    /// mpv's selected track changed. Carries no track id on purpose: this is
    /// handled a whole file load later, so the runtime re-reads `aid`/`sid`.
    TrackChanged(TrackKind),
    Exited,
}

impl MpvEvent {
    /// `None` for an event the runtime does not act on.
    pub(super) fn parse(name: &str, message: &Value) -> Option<Self> {
        match name {
            "end-file" => Some(Self::EndFile {
                reason: EndFileReason::parse(message.get("reason").and_then(Value::as_str)),
            }),
            "file-loaded" => Some(Self::FileLoaded),
            "property-change" => {
                let property = message.get("name").and_then(Value::as_str)?;
                [TrackKind::Audio, TrackKind::Subtitle]
                    .into_iter()
                    .find(|kind| kind.mpv_property() == property)
                    .map(Self::TrackChanged)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "event_test.rs"]
mod tests;
