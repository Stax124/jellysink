use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

fn pending_entry() -> (Reply, oneshot::Receiver<Result<Value, String>>) {
    oneshot::channel()
}

#[tokio::test]
async fn ipc_roundtrip_against_fake_socket() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("mpv.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["command"][0], "get_property");
        let id = v["request_id"].as_i64().unwrap();
        let reply = format!(
            "{}\n",
            json!({"error":"success","data":true,"request_id": id})
        );
        reader.get_mut().write_all(reply.as_bytes()).await.unwrap();
        // Hold the socket open until the client closes it.
        let mut rest = String::new();
        let _ = reader.read_line(&mut rest).await;
    });

    let stream = UnixStream::connect(&sock).await.unwrap();
    let (requests, request_rx) = mpsc::unbounded_channel();
    let (event_tx, _events) = mpsc::unbounded_channel();
    let ipc = tokio::spawn(ipc_loop(stream, request_rx, event_tx));

    let (reply, answer) = oneshot::channel();
    requests
        .send(Request {
            line: encode_command(1, &[json!("get_property"), json!("pause")]),
            id: 1,
            reply,
        })
        .unwrap();
    let data = answer.await.unwrap().unwrap();
    assert_eq!(data, json!(true));
    // No more requests can come, which ends the loop and closes the socket.
    drop(requests);
    ipc.await.unwrap();
    server.await.unwrap();
}

#[test]
fn abandoned_requests_are_evicted() {
    let mut pending = HashMap::new();
    let (live, _live_rx) = pending_entry();
    let (abandoned, abandoned_rx) = pending_entry();
    pending.insert(1, live);
    pending.insert(2, abandoned);

    // The caller timed out and dropped its receiver.
    drop(abandoned_rx);

    assert_eq!(evict_abandoned(&mut pending), 1);
    assert!(pending.contains_key(&1), "a live waiter must be kept");
    assert!(!pending.contains_key(&2));
}

#[test]
fn evicting_leaves_a_map_of_live_waiters_alone() {
    let mut pending = HashMap::new();
    let (a, _a_rx) = pending_entry();
    let (b, _b_rx) = pending_entry();
    pending.insert(1, a);
    pending.insert(2, b);
    assert_eq!(evict_abandoned(&mut pending), 0);
    assert_eq!(pending.len(), 2);
}

#[test]
fn a_command_label_never_carries_a_url() {
    let loadfile = [
        json!("loadfile"),
        json!("http://s/Videos/i/stream?ApiKey=secret"),
        json!("replace"),
    ];
    assert_eq!(command_label(&loadfile), "loadfile");
    assert_eq!(
        command_label(&[json!("set_property"), json!("keep-open"), json!("yes")]),
        "set_property keep-open"
    );
}
