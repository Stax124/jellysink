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
