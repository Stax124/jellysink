use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[test]
fn encode_is_one_json_line() {
    let line = encode_command(3, &[json!("get_property"), json!("pause")]);
    assert!(line.ends_with('\n'));
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["request_id"], 3);
    assert_eq!(v["command"][0], "get_property");
}

#[test]
fn playlist_m3u_strips_newlines_from_title() {
    let body = playlist_m3u([("A\nB\rC", "http://h/x")]);
    assert_eq!(body, "#EXTM3U\n#EXTINF:-1,A B C\nhttp://h/x\n");
}

#[test]
fn playlist_m3u_writes_every_entry() {
    let body = playlist_m3u([
        ("Show - s1e01 - One", "http://h/a"),
        ("Show - s1e02 - Two", "http://h/b"),
    ]);
    assert_eq!(
        body,
        "#EXTM3U\n#EXTINF:-1,Show - s1e01 - One\nhttp://h/a\n#EXTINF:-1,Show - s1e02 - Two\nhttp://h/b\n"
    );
}

#[test]
fn loadlist_append_is_path_and_append_only() {
    let line = encode_command(1, &loadlist_append_args("/tmp/append.m3u"));
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    let cmd = v["command"].as_array().unwrap();
    assert_eq!(cmd.len(), 3);
    assert_eq!(cmd[0], "loadlist");
    assert_eq!(cmd[1], "/tmp/append.m3u");
    assert_eq!(cmd[2], "append");
}

#[test]
fn loadlist_insert_at_keeps_the_index_a_separate_argument() {
    // "insert-at0" as one token is `invalid parameter` in mpv.
    let line = encode_command(1, &loadlist_insert_at_args("/tmp/insert.m3u", 0));
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    let cmd = v["command"].as_array().unwrap();
    assert_eq!(cmd.len(), 4);
    assert_eq!(cmd[0], "loadlist");
    assert_eq!(cmd[1], "/tmp/insert.m3u");
    assert_eq!(cmd[2], "insert-at");
    assert_eq!(cmd[3], 0);
}

#[test]
fn loadlist_insert_at_uses_the_given_index() {
    let line = encode_command(1, &loadlist_insert_at_args("/tmp/insert.m3u", 3));
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    let cmd = v["command"].as_array().unwrap();
    assert_eq!(cmd[3], 3);
}

#[test]
fn time_pos_parses_integer_and_float_seconds() {
    assert_eq!(json_as_seconds(&json!(12)), Some(12.0));
    assert_eq!(json_as_seconds(&json!(12.5)), Some(12.5));
}

#[test]
fn time_pos_rejects_a_non_number_instead_of_zero() {
    assert_eq!(json_as_seconds(&json!(null)), None);
    assert_eq!(json_as_seconds(&json!("unavailable")), None);
}

#[test]
fn parse_reply_and_event() {
    let r = parse_ipc_line(r#"{"error":"success","data":12.5,"request_id":1}"#).unwrap();
    assert_eq!(
        r,
        IpcMessage::Reply {
            request_id: 1,
            error: "success".into(),
            data: json!(12.5),
        }
    );
    let e = parse_ipc_line(r#"{"event":"end-file","reason":"eof"}"#).unwrap();
    assert_eq!(
        e,
        IpcMessage::Event {
            name: "end-file".into(),
            reason: Some("eof".into()),
        }
    );
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
fn a_property_change_parses_to_the_property_name() {
    assert_eq!(
        parse_ipc_line(r#"{"event":"property-change","id":1,"name":"sid","data":3}"#).unwrap(),
        IpcMessage::PropertyChange {
            property: "sid".into()
        }
    );
}

#[test]
fn only_the_observed_track_properties_become_events() {
    assert!(matches!(
        mpv_event_for(&IpcMessage::PropertyChange {
            property: SUBTITLE_TRACK_PROPERTY.into()
        }),
        Some(MpvEvent::SubtitleTrackChanged)
    ));
    assert!(matches!(
        mpv_event_for(&IpcMessage::PropertyChange {
            property: AUDIO_TRACK_PROPERTY.into()
        }),
        Some(MpvEvent::AudioTrackChanged)
    ));
    // Polled every second rather than observed; a change event for it would
    // be a property we never asked about.
    assert!(
        mpv_event_for(&IpcMessage::PropertyChange {
            property: "volume".into()
        })
        .is_none()
    );
}

/// The whole point of [`SelectedTrack`]: `no` and `auto` are different
/// answers, and reading `auto` as "off" would record a file that is still
/// loading as the user switching subtitles off.
#[test]
fn a_track_property_tells_off_apart_from_not_yet_decided() {
    assert_eq!(
        selected_track_from_property(&json!(3)),
        SelectedTrack::Id(3)
    );
    assert_eq!(
        selected_track_from_property(&json!(false)),
        SelectedTrack::Off
    );
    assert_eq!(
        selected_track_from_property(&json!("no")),
        SelectedTrack::Off
    );
    assert_eq!(
        selected_track_from_property(&json!("auto")),
        SelectedTrack::Unresolved
    );
    assert_eq!(
        selected_track_from_property(&Value::Null),
        SelectedTrack::Unresolved
    );
}

#[test]
fn observe_property_sends_an_id_and_the_property_name() {
    for (id, property) in [
        (SUBTITLE_TRACK_OBSERVER_ID, SUBTITLE_TRACK_PROPERTY),
        (AUDIO_TRACK_OBSERVER_ID, AUDIO_TRACK_PROPERTY),
    ] {
        let line = encode_command(7, &[json!("observe_property"), json!(id), json!(property)]);
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let cmd = v["command"].as_array().unwrap();
        assert_eq!(cmd[0], "observe_property");
        assert_eq!(cmd[1], id);
        assert_eq!(cmd[2], property);
    }
    // mpv echoes the id back on every change, so two observers must not
    // share one.
    assert_ne!(SUBTITLE_TRACK_OBSERVER_ID, AUDIO_TRACK_OBSERVER_ID);
}

#[test]
fn max_subtitle_track_id_from_track_list_picks_the_highest_sub_id() {
    let list = json!([
        {"type": "audio", "id": 1},
        {"type": "sub", "id": 1},
        {"type": "sub", "id": 3},
        {"type": "video", "id": 1}
    ]);
    assert_eq!(max_subtitle_track_id_from_track_list(&list), 3);
}

#[test]
fn max_subtitle_track_id_from_track_list_is_zero_when_empty() {
    assert_eq!(max_subtitle_track_id_from_track_list(&json!([])), 0);
    assert_eq!(max_subtitle_track_id_from_track_list(&json!(null)), 0);
}

#[test]
fn property_coercions_accept_the_expected_shapes() {
    assert_eq!(as_i64_property("playlist-pos", &json!(3)).unwrap(), 3);
    // mpv reports -1 for playlist-pos while idle; that is a real answer.
    assert_eq!(as_i64_property("playlist-pos", &json!(-1)).unwrap(), -1);
    assert_eq!(as_f64_property("volume", &json!(62.5)).unwrap(), 62.5);
    assert!(as_bool_property("pause", &json!(true)).unwrap());
}

/// The regression: a null or wrong-typed answer used to become 0 / 100.0 /
/// false, and `playlist_eof` then picked an episode from it.
#[test]
fn property_coercions_reject_a_missing_or_wrong_typed_answer() {
    for v in [json!(null), json!("3"), json!({})] {
        assert!(
            as_i64_property("playlist-pos", &v).is_err(),
            "{v} should not coerce to an integer"
        );
    }
    assert!(as_f64_property("volume", &json!(null)).is_err());
    assert!(as_bool_property("pause", &json!(null)).is_err());
    assert!(as_bool_property("pause", &json!(1)).is_err());
}

#[test]
fn a_coercion_error_names_the_property_and_what_arrived() {
    let err = as_i64_property("playlist-count", &json!("nope")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("playlist-count"), "{msg}");
    assert!(msg.contains("nope"), "{msg}");
}

fn pending_entry() -> (Pending, oneshot::Receiver<Result<Value, String>>) {
    let (tx, rx) = oneshot::channel();
    (Pending { tx }, rx)
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

#[tokio::test]
async fn playlist_files_are_created_private_not_chmodded_after() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("append.m3u");
    write_private(&path, "#EXTM3U\n").await.unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the body can carry ApiKey=");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "#EXTM3U\n");
}

/// A file left behind by a crash must not donate its looser mode.
#[tokio::test]
async fn a_stale_world_readable_playlist_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("insert.m3u");
    std::fs::write(&path, "stale").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    write_private(&path, "fresh").await.unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh");
}

#[test]
fn end_file_reasons_parse_to_their_variants() {
    assert_eq!(EndFileReason::parse(Some("eof")), EndFileReason::Eof);
    assert_eq!(
        EndFileReason::parse(Some("redirect")),
        EndFileReason::Redirect
    );
    assert_eq!(EndFileReason::parse(Some("stop")), EndFileReason::Stop);
    assert_eq!(EndFileReason::parse(Some("quit")), EndFileReason::Quit);
    assert_eq!(EndFileReason::parse(Some("error")), EndFileReason::Error);
}

#[test]
fn an_unknown_or_missing_reason_becomes_other() {
    assert_eq!(EndFileReason::parse(None), EndFileReason::Other);
    assert_eq!(
        EndFileReason::parse(Some("something-new")),
        EndFileReason::Other
    );
}

#[test]
fn display_round_trips_mpv_spelling() {
    for name in ["eof", "redirect", "stop", "quit", "error"] {
        assert_eq!(EndFileReason::parse(Some(name)).to_string(), name);
    }
    assert_eq!(EndFileReason::Other.to_string(), "unknown");
}
