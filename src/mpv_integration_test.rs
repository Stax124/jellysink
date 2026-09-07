//! Integration tests that drive a real mpv process over its real IPC socket.
//!
//! mpv is forced headless here (`--vo=null --ao=null`) and given `--no-config`.
//! That is the opposite of what the daemon does on purpose — the product
//! promise is the *user's* mpv config and video output (see `AGENTS.md`), but a
//! test that inherited either would fail differently on every machine.

use super::*;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

/// How long a property is given to reach the value a command asked for. mpv
/// applies most of them within a frame; this is generous so a loaded CI box
/// does not fail the suite.
const SETTLE: Duration = Duration::from_secs(5);

macro_rules! require_mpv {
    () => {
        assert!(
            mpv_is_installed(),
            "mpv is not on PATH; these tests drive a real player -- install mpv"
        );
    };
}

/// Polls until `$cond` holds, or fails the test naming what never happened.
/// mpv acknowledges a `set_property` before the player has necessarily acted on
/// it, so reading straight back is racy.
///
/// A macro rather than a method taking a closure: an `async` closure borrowing
/// the session cannot name the lifetime it returns.
macro_rules! wait_until {
    ($mpv:expr, $what:expr, |$session:ident| $cond:expr) => {{
        let deadline = Instant::now() + SETTLE;
        loop {
            let $session = &mut $mpv.session;
            if $cond {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{} did not happen within {SETTLE:?}",
                $what
            );
            sleep(Duration::from_millis(25)).await;
        }
    }};
}

fn mpv_is_installed() -> bool {
    std::process::Command::new("mpv")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    assert!(
        path.exists(),
        "missing fixture {}; run tests/fixtures/make-fixtures.sh",
        path.display()
    );
    path.to_string_lossy().into_owned()
}

/// Dropping it kills mpv, so no test has to clean up after a failed assertion.
struct TestMpv {
    session: MpvSession,
    events: mpsc::UnboundedReceiver<MpvEvent>,
    _socket_dir: tempfile::TempDir,
}

impl TestMpv {
    async fn start() -> Self {
        let socket_dir = tempfile::tempdir().unwrap();
        let args: Vec<String> = ["--no-config", "--vo=null", "--ao=null", "--really-quiet"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        let (session, events) = MpvSession::spawn("mpv", &args, socket_dir.path().join("mpv.sock"))
            .await
            .expect("spawning mpv");
        Self {
            session,
            events,
            _socket_dir: socket_dir,
        }
    }

    /// Loads the fixture and waits for mpv to report it loaded, so a following
    /// assertion sees a file with tracks rather than an idle player.
    async fn play_fixture(&mut self) {
        self.session
            .loadfile(&fixture("sample.mkv"), None)
            .await
            .expect("loadfile");
        self.wait_for_event(|event| matches!(event, MpvEvent::FileLoaded))
            .await;
    }

    async fn wait_for_event(&mut self, want: impl Fn(&MpvEvent) -> bool) -> MpvEvent {
        let deadline = Instant::now() + SETTLE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match timeout(remaining, self.events.recv()).await {
                Ok(Some(event)) if want(&event) => return event,
                Ok(Some(_)) => continue,
                Ok(None) => panic!("mpv event stream closed while waiting"),
                Err(_) => panic!("no matching mpv event within {SETTLE:?}"),
            }
        }
    }

    /// Can only ever show that mpv has not spoken *yet*, so keep `window` short.
    async fn expect_no_event(&mut self, window: Duration) {
        if let Ok(Some(event)) = timeout(window, self.events.recv()).await {
            panic!("unexpected mpv event: {event:?}");
        }
    }

    /// Starts `title` the way the runtime starts an item -- `loadfile ...
    /// replace`, which wipes the playlist -- and waits for it to load. Queue
    /// entries are appended around it afterwards, as `queue.rs` does.
    async fn start_current(&mut self, title: &str) {
        self.session
            .loadfile(&fixture("sample.mkv"), Some(title))
            .await
            .expect("loadfile");
        self.wait_for_event(|event| matches!(event, MpvEvent::FileLoaded))
            .await;
    }
}

#[tokio::test]
async fn mpv_starts_idle_and_answers_property_queries() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;

    assert!(!mpv.session.paused().await.unwrap());
    assert!(!mpv.session.muted().await.unwrap());
    assert_eq!(mpv.session.volume().await.unwrap(), 100);
    // Idle: no playlist entry is current, which mpv reports as -1.
    assert_eq!(mpv.session.playlist_pos().await.unwrap(), -1);
    assert_eq!(mpv.session.playlist_count().await.unwrap(), 0);
}

#[tokio::test]
async fn loading_a_file_reports_it_loaded_and_playing() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.play_fixture().await;

    wait_until!(mpv, "time-pos advances", |session| session
        .time_pos()
        .await
        .is_ok_and(|position| position > 0.0));
    assert_eq!(mpv.session.playlist_pos().await.unwrap(), 0);
    assert_eq!(mpv.session.playlist_count().await.unwrap(), 1);
}

#[tokio::test]
async fn a_loadfile_title_becomes_the_media_title() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.session
        .loadfile(&fixture("sample.mkv"), Some("S01E01 - Pilot"))
        .await
        .unwrap();
    mpv.wait_for_event(|event| matches!(event, MpvEvent::FileLoaded))
        .await;

    let title = mpv.session.get_property("media-title").await.unwrap();
    assert_eq!(title, json!("S01E01 - Pilot"));
}

#[tokio::test]
async fn pause_unpause_and_toggle_round_trip_through_mpv() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.play_fixture().await;

    mpv.session.pause().await.unwrap();
    assert!(mpv.session.paused().await.unwrap());

    mpv.session.toggle_pause().await.unwrap();
    assert!(!mpv.session.paused().await.unwrap());

    mpv.session.toggle_pause().await.unwrap();
    assert!(mpv.session.paused().await.unwrap());

    mpv.session.unpause().await.unwrap();
    assert!(!mpv.session.paused().await.unwrap());
}

#[tokio::test]
async fn seeking_moves_the_position() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.play_fixture().await;
    // Paused, so the assertion below is not racing playback.
    mpv.session.pause().await.unwrap();

    mpv.session.seek_absolute(2.0).await.unwrap();
    wait_until!(mpv, "seek to 2s", |session| session
        .time_pos()
        .await
        .is_ok_and(|position| (1.5..2.5).contains(&position)));

    mpv.session.seek_absolute(0.0).await.unwrap();
    wait_until!(mpv, "seek back to 0s", |session| session
        .time_pos()
        .await
        .is_ok_and(|position| position < 0.5));
}

#[tokio::test]
async fn volume_and_mute_round_trip_and_clamp_at_the_ends() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;

    mpv.session.set_volume(40).await.unwrap();
    assert_eq!(mpv.session.volume().await.unwrap(), 40);

    assert_eq!(mpv.session.add_volume(15).await.unwrap(), 55);
    assert_eq!(mpv.session.volume().await.unwrap(), 55);

    assert_eq!(mpv.session.add_volume(-500).await.unwrap(), 0);
    assert_eq!(mpv.session.volume().await.unwrap(), 0);
    assert_eq!(mpv.session.add_volume(500).await.unwrap(), 100);
    assert_eq!(mpv.session.volume().await.unwrap(), 100);

    mpv.session.set_mute(true).await.unwrap();
    assert!(mpv.session.muted().await.unwrap());
    mpv.session.set_mute(false).await.unwrap();
    assert!(!mpv.session.muted().await.unwrap());
}

#[tokio::test]
async fn audio_and_subtitle_tracks_select_by_id_and_turn_off() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.play_fixture().await;

    // The fixture carries two of each; ids are 1 and 2 per kind.
    mpv.session.set_audio_track_id(Some(2)).await.unwrap();
    wait_until!(mpv, "aid becomes 2", |session| session
        .audio_track()
        .await
        .unwrap()
        == SelectedTrack::Id(2));
    mpv.session.set_subtitle_track_id(Some(2)).await.unwrap();
    wait_until!(mpv, "sid becomes 2", |session| session
        .subtitle_track()
        .await
        .unwrap()
        == SelectedTrack::Id(2));

    // `None` is off, and off must read back as Off rather than Unresolved --
    // the distinction the runtime uses to tell a decision from a loading file.
    mpv.session.set_audio_track_id(None).await.unwrap();
    wait_until!(mpv, "aid turns off", |session| session
        .audio_track()
        .await
        .unwrap()
        == SelectedTrack::Off);
    mpv.session.set_subtitle_track_id(None).await.unwrap();
    wait_until!(mpv, "sid turns off", |session| session
        .subtitle_track()
        .await
        .unwrap()
        == SelectedTrack::Off);
}

#[tokio::test]
async fn a_negative_track_id_is_off_not_a_track() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.play_fixture().await;

    // Where `cycle audio` lands after the last track.
    mpv.session.set_audio_track_id(Some(-1)).await.unwrap();
    wait_until!(mpv, "aid turns off", |session| session
        .audio_track()
        .await
        .unwrap()
        == SelectedTrack::Off);
}

#[tokio::test]
async fn observed_track_properties_report_a_change_back() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.session.observe_audio_track().await.unwrap();
    mpv.session.observe_subtitle_track().await.unwrap();
    mpv.play_fixture().await;

    mpv.session.set_audio_track_id(Some(2)).await.unwrap();
    mpv.wait_for_event(|event| matches!(event, MpvEvent::AudioTrackChanged))
        .await;

    mpv.session.set_subtitle_track_id(Some(2)).await.unwrap();
    mpv.wait_for_event(|event| matches!(event, MpvEvent::SubtitleTrackChanged))
        .await;
}

#[tokio::test]
async fn an_added_subtitle_gets_the_next_track_id() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.play_fixture().await;

    // The fixture ships two embedded subtitle tracks, so 2 is the id an added
    // one has to come after.
    assert_eq!(mpv.session.max_subtitle_track_id().await.unwrap(), 2);

    mpv.session.sub_add(&fixture("external.srt")).await.unwrap();
    assert_eq!(mpv.session.max_subtitle_track_id().await.unwrap(), 3);

    // A higher count alone would not be enough: the runtime picks the external
    // subtitle by this id, so the id has to address the file we added.
    mpv.session.set_subtitle_track_id(Some(3)).await.unwrap();
    wait_until!(mpv, "sid becomes the added track", |session| session
        .subtitle_track()
        .await
        .unwrap()
        == SelectedTrack::Id(3));
}

/// mpv's `playlist` property as `(title, filename)`, which is what the M3U
/// entries have to survive as.
async fn playlist_entries(session: &mut MpvSession) -> Vec<(Option<String>, String)> {
    let list = session.get_property("playlist").await.unwrap();
    list.as_array()
        .expect("playlist is an array")
        .iter()
        .map(|entry| {
            (
                entry
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                entry
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

async fn playlist_titles(session: &mut MpvSession) -> Vec<Option<String>> {
    playlist_entries(session)
        .await
        .into_iter()
        .map(|(title, _)| title)
        .collect()
}

#[tokio::test]
async fn an_appended_playlist_keeps_its_extinf_titles_and_order() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let file = fixture("sample.mkv");

    mpv.session
        .loadlist_append(&[("First Episode", &file), ("Second Episode", &file)])
        .await
        .unwrap();

    assert_eq!(mpv.session.playlist_count().await.unwrap(), 2);
    // Appending to an idle mpv queues the entries and starts nothing: the
    // runtime has to `loadfile` the item it wants playing.
    assert_eq!(mpv.session.playlist_pos().await.unwrap(), -1);
    let entries = playlist_entries(&mut mpv.session).await;
    assert_eq!(
        entries
            .iter()
            .map(|(title, _)| title.as_deref())
            .collect::<Vec<_>>(),
        [Some("First Episode"), Some("Second Episode")]
    );

    // The M3U itself is temporary: it is removed once mpv has read it.
    assert!(!mpv.session.socket.with_file_name("append.m3u").exists());
}

#[tokio::test]
async fn inserting_into_a_playlist_leaves_the_playing_entry_alone() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let file = fixture("sample.mkv");
    mpv.start_current("First").await;

    mpv.session
        .loadlist_append(&[("Third", &file)])
        .await
        .unwrap();
    mpv.session
        .loadlist_insert_at(&[("Second", &file)], 1)
        .await
        .unwrap();

    assert_eq!(mpv.session.playlist_count().await.unwrap(), 3);
    // `force-media-title` names the current entry, `#EXTINF` the queued ones,
    // and mpv reports both the same way.
    assert_eq!(
        playlist_titles(&mut mpv.session).await,
        ["First", "Second", "Third"].map(|title| Some(title.to_owned()))
    );
    assert_eq!(mpv.session.playlist_pos().await.unwrap(), 0);
    mpv.expect_no_event(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn inserting_before_the_current_entry_shifts_the_position() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let file = fixture("sample.mkv");
    mpv.start_current("A").await;
    mpv.session.loadlist_append(&[("B", &file)]).await.unwrap();

    mpv.session
        .loadlist_insert_at(&[("Before", &file)], 0)
        .await
        .unwrap();

    // mpv keeps playing the same entry, so its index moves -- the shift
    // `PlaylistWindow` accounts for when it prepends (see specs/playlist.md).
    assert_eq!(mpv.session.playlist_pos().await.unwrap(), 1);
    assert_eq!(
        playlist_titles(&mut mpv.session).await,
        ["Before", "A", "B"].map(|title| Some(title.to_owned()))
    );
}

#[tokio::test]
async fn playlist_next_and_prev_move_the_position() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let file = fixture("sample.mkv");
    mpv.start_current("A").await;
    mpv.session.loadlist_append(&[("B", &file)]).await.unwrap();

    mpv.session.playlist_next().await.unwrap();
    wait_until!(mpv, "playlist-pos becomes 1", |session| session
        .playlist_pos()
        .await
        .unwrap()
        == 1);

    mpv.session.playlist_prev().await.unwrap();
    wait_until!(mpv, "playlist-pos becomes 0", |session| session
        .playlist_pos()
        .await
        .unwrap()
        == 0);
}

#[tokio::test]
async fn moving_off_an_entry_ends_the_file_with_stop() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let file = fixture("sample.mkv");
    mpv.start_current("A").await;
    mpv.session.loadlist_append(&[("B", &file)]).await.unwrap();

    mpv.session.playlist_next().await.unwrap();

    // `stop`, not `eof`: `end_file_action` must not read a skip as the item
    // having been watched to the end.
    let event = mpv
        .wait_for_event(|event| matches!(event, MpvEvent::EndFile { .. }))
        .await;
    assert!(
        matches!(
            event,
            MpvEvent::EndFile {
                reason: EndFileReason::Stop
            }
        ),
        "expected stop, got {event:?}"
    );
}

#[tokio::test]
async fn playing_to_the_end_of_a_queued_item_ends_with_eof_and_advances() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.session.set_keep_open().await.unwrap();
    mpv.start_current("A").await;
    mpv.session
        .loadlist_append(&[("B", &fixture("sample.mkv"))])
        .await
        .unwrap();

    mpv.session.seek_absolute(2.9).await.unwrap();

    let event = mpv
        .wait_for_event(|event| matches!(event, MpvEvent::EndFile { .. }))
        .await;
    assert!(
        matches!(
            event,
            MpvEvent::EndFile {
                reason: EndFileReason::Eof
            }
        ),
        "expected eof, got {event:?}"
    );
    // `KEEP_OPEN` is `yes`, so mpv still autoplays the rest of the playlist.
    wait_until!(mpv, "playlist-pos becomes 1", |session| session
        .playlist_pos()
        .await
        .unwrap()
        == 1);
}

#[tokio::test]
async fn keep_open_holds_the_last_item_and_says_nothing() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    mpv.session.set_keep_open().await.unwrap();
    mpv.start_current("Only").await;

    mpv.session.seek_absolute(2.9).await.unwrap();
    // Nothing announces the end of the last item, so there is nothing to wait
    // on but the clock.
    sleep(Duration::from_secs(2)).await;

    // With nothing left to autoplay, `keep-open` holds the window at the last
    // frame and mpv emits no `end-file` at all -- the runtime therefore cannot
    // learn the item finished from an event, and must not wait for one.
    assert_eq!(
        mpv.session.get_property("eof-reached").await.unwrap(),
        json!(true)
    );
    mpv.expect_no_event(Duration::from_millis(500)).await;
    assert_eq!(mpv.session.playlist_pos().await.unwrap(), 0);
}

#[tokio::test]
async fn a_missing_file_ends_with_error_rather_than_killing_the_session() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let missing = mpv.session.socket.with_file_name("nope.mkv");

    mpv.session
        .loadfile(&missing.to_string_lossy(), None)
        .await
        .unwrap();

    let event = mpv
        .wait_for_event(|event| matches!(event, MpvEvent::EndFile { .. }))
        .await;
    assert!(
        matches!(
            event,
            MpvEvent::EndFile {
                reason: EndFileReason::Error
            }
        ),
        "expected error, got {event:?}"
    );
    assert!(mpv.session.volume().await.is_ok(), "session still usable");
}

#[tokio::test]
async fn quitting_stops_the_process_and_removes_the_socket() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;
    let socket = mpv.session.socket.clone();
    assert!(socket.exists());

    mpv.session.quit_and_wait().await.unwrap();

    assert!(!socket.exists(), "the IPC socket outlived mpv");
    assert!(mpv.session.volume().await.is_err(), "mpv still answering");
}

#[tokio::test]
async fn the_socket_is_not_world_readable() {
    require_mpv!();
    let mpv = TestMpv::start().await;

    // mpv creates it under the ambient umask; `spawn` narrows it, because
    // `http-header-fields` on it carries the Jellyfin access token.
    let mode = std::fs::metadata(&mpv.session.socket)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "socket mode was {mode:o}");
}

/// A `404` is enough: the test only needs to see what mpv put on the wire.
async fn capture_one_request(listener: TcpListener) -> String {
    let (mut stream, _) = listener.accept().await.expect("accept");
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    let _ = stream
        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
        .await;
    String::from_utf8_lossy(&head).into_owned()
}

#[tokio::test]
async fn the_auth_header_is_sent_on_http_requests_and_can_be_cleared() {
    require_mpv!();
    let mut mpv = TestMpv::start().await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/video.mkv", listener.local_addr().unwrap());
    let server = tokio::spawn(capture_one_request(listener));

    mpv.session
        .apply_auth_header("Authorization: MediaBrowser Token=\"secret\"")
        .await
        .unwrap();
    mpv.session.loadfile(&url, None).await.unwrap();

    let head = timeout(SETTLE, server)
        .await
        .expect("mpv never connected")
        .unwrap();
    assert!(
        head.contains("Authorization: MediaBrowser Token=\"secret\""),
        "header missing from request:\n{head}"
    );

    // Cleared, the next request must not carry it -- playlist stubs mpv writes
    // to watch_later files are built on that.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/video.mkv", listener.local_addr().unwrap());
    let server = tokio::spawn(capture_one_request(listener));

    mpv.session.clear_auth_header().await.unwrap();
    mpv.session.loadfile(&url, None).await.unwrap();

    let head = timeout(SETTLE, server)
        .await
        .expect("mpv never connected")
        .unwrap();
    assert!(
        !head.contains("Authorization:"),
        "header survived the clear:\n{head}"
    );
}
