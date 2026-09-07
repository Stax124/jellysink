//! mpv's playlist is the contiguous window
//! `items[origin .. origin + head + 1 + tail]`, and the current item sits at
//! `index - origin`. These tests pin that invariant across a prepend, against
//! the real `PlaylistWindow` rather than a model of it.

use super::*;
use serde_json::json;

/// `start_current`: mpv holds only the current item.
fn start(items: &[&str], index: usize) -> PlaylistWindow {
    let mut w = PlaylistWindow::default();
    // `replace`, not `queue` directly: it also rebuilds NowPlayingQueue.
    w.replace(items.iter().map(|s| s.to_string()).collect(), index);
    w.reset_to_current();
    w
}

fn prepend(w: &mut PlaylistWindow, ids: &[&str]) {
    w.prepend(ids.iter().map(|s| s.to_string()).collect());
}

/// The item mpv is playing, per the window's own arithmetic.
fn current(w: &PlaylistWindow) -> &str {
    &w.queue.items[w.origin() + w.expected_pos()]
}

#[test]
fn prepend_keeps_the_window_contiguous_and_current_stable() {
    // Playing e8 of a 13-episode series; 8..13 already queued.
    let mut w = start(&["e8", "e9", "e10", "e11", "e12", "e13"], 0);
    assert_eq!(current(&w), "e8");

    prepend(&mut w, &["e1", "e2", "e3", "e4", "e5", "e6", "e7"]);

    assert_eq!(current(&w), "e8", "current item must not move");
    assert_eq!(w.head(), 7);
    assert_eq!(w.origin(), 0, "origin is unchanged by a prepend");
    assert_eq!(
        w.mpv_playlist(),
        ["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8"]
    );
}

#[test]
fn prepend_then_fill_appends_after_the_current_item() {
    let mut w = start(&["e8", "e9", "e10"], 0);
    prepend(&mut w, &["e6", "e7"]);
    // Two forward entries land in mpv.
    w.note_appended(2);

    assert_eq!(current(&w), "e8");
    assert_eq!(
        w.mpv_playlist(),
        ["e6", "e7", "e8", "e9", "e10"],
        "the window must stay contiguous across head and tail"
    );
    assert_eq!(
        w.origin() + w.head() + 1 + w.tail(),
        w.queue.items.len(),
        "queue exhausted"
    );
    assert!(w.forward_ids().is_empty());
}

#[test]
fn prepend_from_a_mid_series_start_index() {
    // Jellyfin sent 8..13 with StartIndex=2, so we start on e10.
    let mut w = start(&["e8", "e9", "e10", "e11"], 2);
    assert_eq!(current(&w), "e10");
    assert_eq!(w.origin(), 2);

    prepend(
        &mut w,
        &["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9"],
    );

    assert_eq!(current(&w), "e10", "current item must not move");
    assert_eq!(w.origin(), 2, "origin is unchanged by a prepend");
    assert_eq!(w.head(), 9);
    assert_eq!(
        w.mpv_playlist(),
        ["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9", "e10"]
    );
}

#[test]
fn prepend_happens_even_though_jellyfin_sent_a_full_queue() {
    // Reproduces the reported bug: Jellyfin sends e6..e20, so has_next is
    // true and the forward gate bails. The prepend must still run, and the
    // forward append must not duplicate e7..e20.
    let mut w = start(&["e6", "e7", "e8"], 0);
    assert!(w.queue.has_next(), "forward gate would bail here");

    // Full series listing split at e6.
    let (previous, _rest) = split_at(&["e1", "e2", "e3", "e4", "e5", "e6", "e7"], "e6");
    let missing = crate::runtime::queue::ids_missing_from(&previous, &w.queue.items);
    assert_eq!(missing, ["e1", "e2", "e3", "e4", "e5"]);

    w.prepend(missing);

    assert_eq!(current(&w), "e6", "current item must not move");
    assert_eq!(w.origin(), 0);
    assert_eq!(w.head(), 5);
    assert_eq!(
        w.mpv_playlist(),
        ["e1", "e2", "e3", "e4", "e5", "e6"],
        "e7..e20 must not be duplicated by the forward append"
    );
}

#[test]
fn advancing_then_prepending_does_not_duplicate() {
    // After e6 ends we advance to e7; e1..e6 are already queued ahead of
    // it, so re-running the expand must add nothing.
    let mut w = start(&["e6", "e7", "e8"], 0);
    prepend(&mut w, &["e1", "e2", "e3", "e4", "e5"]);
    w.queue.advance();
    assert_eq!(current(&w), "e7");

    let (previous, _rest) = split_at(&["e1", "e2", "e3", "e4", "e5", "e6", "e7"], "e7");
    let missing = crate::runtime::queue::ids_missing_from(&previous, &w.queue.items);
    assert!(
        missing.is_empty(),
        "e1..e6 are already in the queue: {missing:?}"
    );
}

#[test]
fn expected_pos_follows_a_playlist_jump_not_the_head() {
    // Jumping to e7 via the playlist selector moves the queue index but
    // leaves `head` at 5. Deriving the position from `head` would report
    // 5 and misread every subsequent EOF.
    let mut w = start(&["e6", "e7", "e8"], 0);
    prepend(&mut w, &["e1", "e2", "e3", "e4", "e5"]);
    assert_eq!(w.head(), 5);

    w.queue.advance(); // adopt_playlist_pos jumps to e7
    assert_eq!(current(&w), "e7");
    assert_eq!(w.expected_pos(), 6, "position is index - origin, not head");
    assert_ne!(w.expected_pos(), w.head());
}

#[test]
fn eof_expected_pos_is_not_zero_when_previous_episodes_are_loaded() {
    // With previous episodes spliced in, the current item is at mpv
    // position 2, not 0; comparing against 0 would misread every EOF.
    let mut w = start(&["e8", "e9"], 0);
    prepend(&mut w, &["e6", "e7"]);
    w.note_appended(1);

    let count = w.mpv_playlist().len();
    let expected_pos = w.expected_pos();
    assert_eq!(count, 4);
    assert_eq!(expected_pos, 2);
    // mpv already advanced to e9 on EOF: wait rather than playlist-next.
    assert_eq!(
        playlist_eof(3, count, false, expected_pos, true),
        PlaylistEof::WaitForMpv
    );
    // Still on e8 with e9 in mpv: let mpv autoplay it.
    assert_eq!(
        playlist_eof(2, count, true, expected_pos, true),
        PlaylistEof::WaitForMpv
    );
}

#[test]
fn queue_index_at_maps_an_mpv_position_back_and_rejects_one_past_the_end() {
    let mut w = start(&["e6", "e7", "e8"], 0);
    prepend(&mut w, &["e4", "e5"]);
    // mpv position 0 is e4, which is queue index 0.
    assert_eq!(w.queue_index_at(0), Some(0));
    assert_eq!(w.queue_index_at(2), Some(2));
    assert_eq!(w.queue_index_at(5), None, "past the end of the queue");
}

#[test]
fn forward_ids_are_the_entries_mpv_does_not_have_yet() {
    let mut w = start(&["e1", "e2", "e3", "e4"], 0);
    assert_eq!(w.forward_ids(), ["e2", "e3", "e4"]);
    w.note_appended(2);
    assert_eq!(w.forward_ids(), ["e4"]);
}

#[test]
fn reset_to_current_re_anchors_the_window_on_a_playlist_jump() {
    let mut w = start(&["e1", "e2", "e3"], 0);
    w.note_appended(2);
    w.queue.advance();
    w.reset_to_current();
    assert_eq!(w.origin(), 1, "mpv now starts at the new current item");
    assert_eq!(w.expected_pos(), 0);
    assert_eq!(w.head(), 0);
    assert_eq!(w.tail(), 0);
}

#[test]
fn pending_prepend_is_handed_over_exactly_once() {
    let mut w = start(&["e3"], 0);
    prepend(&mut w, &["e1", "e2"]);
    assert_eq!(w.take_pending_prepend(), ["e1", "e2"]);
    assert!(
        w.take_pending_prepend().is_empty(),
        "a second fill must not re-insert them"
    );
}

/// Test helper: split a listing of ids at `current`.
fn split_at(ids: &[&str], current: &str) -> (Vec<String>, Vec<String>) {
    let v = json!({
        "Items": ids.iter().map(|id| json!({"Id": id})).collect::<Vec<_>>()
    });
    crate::runtime::queue::split_episode_ids(&v, current)
}

#[test]
fn queue_play_next_inserts_after_current() {
    let mut q = Queue {
        items: vec!["a".into(), "c".into()],
        index: 0,
    };
    q.insert_next(vec!["b".into()]);
    assert_eq!(q.items, vec!["a", "b", "c"]);
    assert_eq!(q.advance(), Some("b"));
}

#[test]
fn queue_has_next_is_false_on_the_last_item() {
    let mut q = Queue {
        items: vec!["a".into(), "b".into()],
        index: 0,
    };
    assert!(q.has_next());
    assert_eq!(q.advance(), Some("b"));
    assert!(!q.has_next());
    q.append(vec!["c".into()]);
    assert!(q.has_next());
    assert_eq!(q.current(), Some("b"));
}

#[test]
fn end_file_eof_always_tries_the_next_item() {
    assert_eq!(
        end_file_action(false, false, EndFileReason::Eof),
        EndFileAction::Advance
    );
    assert_eq!(
        end_file_action(false, false, EndFileReason::Redirect),
        EndFileAction::Advance
    );
}

#[test]
fn end_file_is_ignored_while_replacing_the_current_file() {
    assert_eq!(
        end_file_action(true, false, EndFileReason::Stop),
        EndFileAction::Ignore
    );
    assert_eq!(
        end_file_action(true, false, EndFileReason::Eof),
        EndFileAction::Ignore
    );
    assert_eq!(
        end_file_action(false, true, EndFileReason::Eof),
        EndFileAction::Ignore
    );
}

#[test]
fn end_file_quit_or_error_stops() {
    assert_eq!(
        end_file_action(false, false, EndFileReason::Quit),
        EndFileAction::Stop
    );
    assert_eq!(
        end_file_action(false, false, EndFileReason::Stop),
        EndFileAction::Stop
    );
    assert_eq!(
        end_file_action(false, false, EndFileReason::Error),
        EndFileAction::Stop
    );
}

#[test]
fn queue_index_tracks_playlist_origin() {
    assert_eq!(queue_index_at(2, 0, 5), Some(2));
    assert_eq!(queue_index_at(2, 2, 5), Some(4));
    assert_eq!(queue_index_at(2, 3, 5), None);
    assert_eq!(queue_index_at(0, 0, 0), None);
}

#[test]
fn eof_uses_mpv_next_when_it_is_already_appended() {
    assert_eq!(playlist_eof(0, 3, true, 0, false), PlaylistEof::NextInMpv);
    assert_eq!(
        playlist_eof(2, 3, true, 2, false),
        PlaylistEof::NextNotInMpv
    );
    assert_eq!(playlist_eof(2, 3, false, 2, false), PlaylistEof::Stop);
    assert_eq!(playlist_eof(0, 1, false, 0, false), PlaylistEof::Stop);
}

#[test]
fn eof_does_not_playlist_next_if_mpv_already_advanced() {
    // keep-open=yes auto-plays N+1 before we handle end-file. pos is already
    // 1 while the ended file was 0; playlist-next would skip to N+2.
    assert_eq!(playlist_eof(1, 3, true, 0, true), PlaylistEof::WaitForMpv);
    assert_eq!(playlist_eof(2, 3, true, 1, true), PlaylistEof::WaitForMpv);
}

#[test]
fn eof_lets_mpv_autoplay_when_next_is_already_appended() {
    // keep-open=yes unloads the current file and starts the next one, which
    // is what emits end-file. playlist-next on top of that skips to N+2;
    // keep-open=always would pause on the last frame and never emit it.
    assert_eq!(playlist_eof(0, 3, true, 0, true), PlaylistEof::WaitForMpv);
    assert_eq!(playlist_eof(2, 3, true, 2, true), PlaylistEof::NextNotInMpv);
    assert_eq!(playlist_eof(2, 3, false, 2, true), PlaylistEof::Stop);
}

#[test]
fn playlist_jump_stop_is_not_a_session_stop() {
    assert!(ignore_stop_for_playlist(EndFileReason::Stop, 3));
    assert!(!ignore_stop_for_playlist(EndFileReason::Stop, 1));
    assert!(!ignore_stop_for_playlist(EndFileReason::Eof, 3));
    assert!(!ignore_stop_for_playlist(EndFileReason::Quit, 3));
    assert!(!ignore_stop_for_playlist(EndFileReason::Error, 2));
}

#[test]
fn insert_before_current_keeps_index_on_the_same_item() {
    let mut q = Queue {
        items: vec!["c".into(), "d".into()],
        index: 0,
    };
    assert_eq!(q.insert_before_current(vec!["a".into(), "b".into()]), 2);
    assert_eq!(q.items, vec!["a", "b", "c", "d"]);
    assert_eq!(q.index, 2);
    assert_eq!(q.current(), Some("c"));
}

#[test]
fn insert_before_current_splices_at_the_index_not_at_zero() {
    // mpv's playlist is items[origin..origin+1+tail]; entries must land
    // inside that window, so a mid-queue insert stays put.
    let mut q = Queue {
        items: vec!["a".into(), "b".into(), "c".into()],
        index: 2,
    };
    assert_eq!(q.insert_before_current(vec!["z".into()]), 1);
    assert_eq!(q.index, 3);
    assert_eq!(q.current(), Some("c"));
    assert_eq!(q.items, vec!["a", "b", "z", "c"]);
}

#[test]
fn insert_before_current_nothing_is_a_no_op() {
    let mut q = Queue {
        items: vec!["a".into()],
        index: 0,
    };
    assert_eq!(q.insert_before_current(vec![]), 0);
    assert_eq!(q.index, 0);
    assert_eq!(q.items, vec!["a"]);
}

#[test]
fn the_now_playing_payload_tracks_the_queue_and_is_shared_not_rebuilt() {
    let mut w = start(&["e1", "e2"], 0);
    let first = w.now_playing_queue();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0]["Id"], "e1");
    assert_eq!(first[1]["PlaylistItemId"], "playlistItem1");

    // Reports come once a second; each must be a refcount bump, not a rebuild.
    assert!(Arc::ptr_eq(&first, &w.now_playing_queue()));

    w.append(vec!["e3".into()]);
    let after = w.now_playing_queue();
    assert_eq!(after.len(), 3, "a queue change must be reflected");
    assert!(!Arc::ptr_eq(&first, &after));
}

#[test]
fn a_prepend_shows_up_in_the_now_playing_payload() {
    let mut w = start(&["e3"], 0);
    prepend(&mut w, &["e1", "e2"]);
    let payload = w.now_playing_queue();
    assert_eq!(payload.len(), 3);
    assert_eq!(payload[0]["Id"], "e1");
    assert_eq!(payload[2]["Id"], "e3");
}
