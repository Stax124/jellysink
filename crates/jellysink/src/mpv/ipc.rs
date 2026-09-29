//! The JSON-per-line protocol on mpv's IPC socket: what a message looks like
//! on the way out, and how an answer coerces on the way back.

use super::event::MpvEvent;
use color_eyre::eyre::{WrapErr, eyre};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IpcMessage {
    /// `Err` carries mpv's own `error` string.
    Reply {
        request_id: i64,
        result: Result<Value, String>,
    },
    Event(Option<MpvEvent>),
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
        return Ok(IpcMessage::Event(MpvEvent::parse(name, &v)));
    }
    let request_id = v
        .get("request_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| eyre!("IPC reply missing request_id"))?;
    let result = match v.get("error").and_then(Value::as_str).unwrap_or("success") {
        "success" => Ok(v.get("data").cloned().unwrap_or(Value::Null)),
        error => Err(error.to_string()),
    };
    Ok(IpcMessage::Reply { request_id, result })
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

#[cfg(test)]
#[path = "ipc_test.rs"]
mod tests;
