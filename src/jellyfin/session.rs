use crate::cast::CastEvent;
use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WsIncoming {
    Cast(CastEvent),
    KeepAlive,
    ForceKeepAlive { seconds: u64 },
    Ignored { message_type: String },
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(rename = "MessageType")]
    message_type: String,
    #[serde(rename = "Data")]
    data: Option<Value>,
}

pub(crate) fn websocket_url(
    server: &str,
    token: &str,
    device_id: &str,
) -> color_eyre::Result<String> {
    let mut url = reqwest::Url::parse(server).wrap_err("server URL")?;
    let scheme = match url.scheme() {
        "https" => "wss",
        _ => "ws",
    };
    url.set_scheme(scheme)
        .map_err(|_| eyre!("could not set websocket scheme"))?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if path == "/" {
        path.clear();
    }
    path.push_str("/socket");
    url.set_path(&path);
    url.query_pairs_mut()
        .clear()
        .append_pair("api_key", token)
        .append_pair("deviceId", device_id);
    Ok(url.to_string())
}

pub(crate) fn parse_ws_message(text: &str) -> color_eyre::Result<WsIncoming> {
    let raw: RawMessage = serde_json::from_str(text).wrap_err("websocket JSON")?;
    Ok(match raw.message_type.as_str() {
        "KeepAlive" => WsIncoming::KeepAlive,
        "ForceKeepAlive" => {
            let seconds = raw
                .data
                .as_ref()
                .and_then(value_as_u64)
                .unwrap_or(60)
                .max(1);
            WsIncoming::ForceKeepAlive { seconds }
        }
        other => match CastEvent::from_ws(other, raw.data.as_ref().unwrap_or(&Value::Null)) {
            Some(ev) => WsIncoming::Cast(ev),
            None => WsIncoming::Ignored {
                message_type: other.to_string(),
            },
        },
    })
}

fn value_as_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| v.as_f64().map(|f| f as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_http() {
        let u = websocket_url("http://h:8096", "tok", "dev").unwrap();
        assert!(u.starts_with("ws://h:8096/socket?"));
        assert!(u.contains("api_key=tok"));
        assert!(u.contains("deviceId=dev"));
    }

    #[test]
    fn ws_url_https_subpath() {
        let u = websocket_url("https://h/jellyfin", "tok", "dev").unwrap();
        assert!(u.starts_with("wss://h/jellyfin/socket?"));
    }

    #[test]
    fn parse_force_keepalive() {
        let m = parse_ws_message(r#"{"MessageType":"ForceKeepAlive","Data":60}"#).unwrap();
        assert_eq!(m, WsIncoming::ForceKeepAlive { seconds: 60 });
    }

    #[test]
    fn parse_unknown_is_ignored() {
        let m = parse_ws_message(r#"{"MessageType":"UserDataChanged","Data":{}}"#).unwrap();
        assert!(matches!(m, WsIncoming::Ignored { .. }));
    }
}
