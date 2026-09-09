use super::super::ipc::encode_command;
use super::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

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
