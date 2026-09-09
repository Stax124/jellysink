use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

fn pending_entry() -> (Pending, oneshot::Receiver<Result<Value, String>>) {
    let (tx, rx) = oneshot::channel();
    (Pending { tx }, rx)
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
        // keep the socket open until the client is done
        sleep(Duration::from_millis(200)).await;
    });

    let stream = UnixStream::connect(&sock).await.unwrap();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    tokio::spawn(ipc_loop(stream, cmd_rx, ev_tx));

    let (tx, rx) = oneshot::channel();
    cmd_tx
        .send(IpcCmd::Request {
            line: encode_command(1, &[json!("get_property"), json!("pause")]),
            id: 1,
            reply: tx,
        })
        .unwrap();
    let data = rx.await.unwrap().unwrap();
    assert_eq!(data, json!(true));
    let _ = cmd_tx.send(IpcCmd::Shutdown);
    let _ = server.await;
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
