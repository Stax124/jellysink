use super::*;
use ratatui::layout::Size;
use serde::Deserialize;
use std::time::Duration;

fn age(path: &Path, seconds: u64) {
    let when = SystemTime::now() - Duration::from_secs(seconds);
    fs::File::open(path).unwrap().set_modified(when).unwrap();
}

fn key(item_id: &str, box_: Size) -> CoverKey {
    let item = jellysink_core::jellyfin::model::Item::deserialize(serde_json::json!({
        "Id": item_id,
        "Name": item_id,
        "Type": "Series",
        "ImageTags": { "Primary": "tag" },
    }))
    .unwrap();
    super::super::Covers::new(
        ratatui_image::picker::Picker::halfblocks(),
        CoverDisk::disabled(),
    )
    .key(&item, box_)
    .unwrap()
}

#[test]
fn a_cover_written_in_one_session_is_read_back_in_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let key = key("s1", Size::new(10, 8));

    CoverDisk::new(dir.path().to_path_buf(), 8).write(&key, b"webp bytes");

    let next_session = CoverDisk::new(dir.path().to_path_buf(), 8);
    assert_eq!(next_session.read(&key).as_deref(), Some(&b"webp bytes"[..]));
}

#[test]
fn two_boxes_inside_one_bucket_share_the_request_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let disk = CoverDisk::new(dir.path().to_path_buf(), 8);
    let small = key("s1", Size::new(10, 8));
    let nudged = key("s1", Size::new(11, 8));
    let far = key("s1", Size::new(40, 30));
    assert_ne!(small, nudged, "a different box is a different encoding");

    disk.write(&small, b"webp bytes");
    assert!(disk.read(&nudged).is_some());
    assert!(disk.read(&far).is_none());
}

#[test]
fn a_lowered_budget_drops_the_cover_left_behind_and_keeps_the_one_being_looked_at() {
    let dir = tempfile::tempdir().unwrap();
    let roomy = CoverDisk::new(dir.path().to_path_buf(), 8);
    let (stale, fresh) = (key("s1", Size::new(10, 8)), key("s2", Size::new(10, 8)));
    let big = vec![0u8; 700 * 1024];

    roomy.write(&stale, &big);
    roomy.write(&fresh, &big);
    // Set rather than slept for: a filesystem's mtime is not always finer than
    // the two writes above.
    age(&roomy.path(&stale), 60);
    age(&roomy.path(&fresh), 30);
    // Reading is what makes a cover recent, so this reverses the two.
    assert!(roomy.read(&stale).is_some());

    let shrunk = CoverDisk::new(dir.path().to_path_buf(), 1);
    shrunk.prune();
    assert!(shrunk.read(&stale).is_some());
    assert!(shrunk.read(&fresh).is_none());
}

#[test]
fn a_session_that_keeps_writing_stays_under_budget_without_being_asked_to() {
    // Nobody calls `prune` here: a long browse must bound itself, or the
    // budget is only honoured at the next startup.
    let dir = tempfile::tempdir().unwrap();
    let disk = CoverDisk::new(dir.path().to_path_buf(), 1);
    let big = vec![0u8; 400 * 1024];

    for index in 0..8 {
        disk.write(&key(&format!("s{index}"), Size::new(10, 8)), &big);
    }

    let total: u64 = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.metadata().unwrap().len())
        .sum();
    assert!(total <= 1024 * 1024, "{total} bytes over a 1 MB budget");
}

#[test]
fn a_zero_budget_writes_nothing_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let disk = CoverDisk::new(dir.path().to_path_buf(), 0);
    let key = key("s1", Size::new(10, 8));

    disk.write(&key, b"webp bytes");
    assert!(disk.read(&key).is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_torn_file_is_discarded_rather_than_re_read_forever() {
    let dir = tempfile::tempdir().unwrap();
    let disk = CoverDisk::new(dir.path().to_path_buf(), 8);
    let key = key("s1", Size::new(10, 8));

    disk.write(&key, b"not an image");
    let bytes = disk.read(&key).expect("the bytes are there to be rejected");
    assert!(
        super::super::encode(
            &bytes,
            &ratatui_image::picker::Picker::halfblocks(),
            key.size
        )
        .is_err()
    );
    disk.discard(&key);
    assert!(disk.read(&key).is_none());
}
