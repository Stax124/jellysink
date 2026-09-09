//! The JSON-per-line protocol on mpv's IPC socket: what a message looks like
//! on the way out, and how an answer coerces on the way back.

use color_eyre::eyre::{WrapErr, eyre};
use serde_json::{Value, json};

/// Inbound IPC message from mpv
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IpcMessage {
    Reply {
        request_id: i64,
        error: String,
        data: Value,
    },
    Event {
        name: String,
        reason: Option<String>,
    },
    /// The new value is dropped — see [`MpvEvent::SubtitleTrackChanged`].
    PropertyChange { property: String },
}

pub(crate) fn encode_command(request_id: i64, args: &[Value]) -> String {
    let v = json!({
        "command": args,
        "request_id": request_id,
    });
    format!("{v}\n")
}

pub(crate) fn parse_ipc_line(line: &str) -> color_eyre::Result<IpcMessage> {
    let v: Value = serde_json::from_str(line.trim()).wrap_err("mpv IPC JSON")?;
    if let Some(name) = v.get("event").and_then(Value::as_str) {
        if name == "property-change" {
            return Ok(IpcMessage::PropertyChange {
                property: v
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        let reason = v.get("reason").and_then(Value::as_str).map(str::to_string);
        return Ok(IpcMessage::Event {
            name: name.to_string(),
            reason,
        });
    }
    let request_id = v
        .get("request_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| eyre!("IPC reply missing request_id"))?;
    let error = v
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("success")
        .to_string();
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    Ok(IpcMessage::Reply {
        request_id,
        error,
        data,
    })
}

/// Coerce an mpv property answer, or say what we actually got. No plausible
/// fallback value: callers make autoplay decisions from these numbers.
pub(super) fn as_i64_property(name: &str, v: &Value) -> color_eyre::Result<i64> {
    v.as_i64()
        .ok_or_else(|| eyre!("mpv property {name:?} was not an integer: {v}"))
}

pub(super) fn as_f64_property(name: &str, v: &Value) -> color_eyre::Result<f64> {
    v.as_f64()
        .ok_or_else(|| eyre!("mpv property {name:?} was not a number: {v}"))
}

pub(super) fn as_bool_property(name: &str, v: &Value) -> color_eyre::Result<bool> {
    v.as_bool()
        .ok_or_else(|| eyre!("mpv property {name:?} was not a boolean: {v}"))
}

pub(crate) fn json_as_seconds(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_u64().map(|n| n as f64))
}

#[cfg(test)]
#[path = "ipc_test.rs"]
mod tests;
