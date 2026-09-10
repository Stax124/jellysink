use super::*;
use serde::Deserialize;

fn item(id: &str, image_tag: Option<&str>) -> Item {
    let mut value = serde_json::json!({ "Id": id, "Name": id, "Type": "Series" });
    if let Some(image_tag) = image_tag {
        value["ImageTags"] = serde_json::json!({ "Primary": image_tag });
    }
    Item::deserialize(value).unwrap()
}

fn key(id: &str) -> CoverKey {
    covers()
        .key(&item(id, Some("tag")), Size::new(2, 2))
        .unwrap()
}

fn covers() -> Covers {
    Covers::new(Picker::halfblocks())
}

fn window(columns_rows: Size, pixels: Size) -> WindowSize {
    WindowSize {
        columns_rows,
        pixels,
    }
}

fn protocol() -> Protocol {
    Picker::halfblocks()
        .new_protocol(
            image::DynamicImage::new_rgb8(4, 4),
            Size::new(2, 2),
            Resize::Fit(None),
        )
        .unwrap()
}

#[test]
fn an_item_with_no_artwork_is_never_asked_for() {
    // Without a tag the request is a guaranteed 404, once per cursor move.
    let covers = covers();
    assert!(covers.key(&item("s1", None), Size::new(4, 4)).is_none());
    assert!(
        covers
            .key(&item("s1", Some("tag")), Size::new(4, 4))
            .is_some()
    );
}

#[test]
fn a_resized_terminal_asks_for_a_new_encoding_instead_of_stretching_the_old() {
    let mut covers = covers();
    let small = covers
        .key(&item("s1", Some("tag")), Size::new(10, 8))
        .unwrap();
    let large = covers
        .key(&item("s1", Some("tag")), Size::new(20, 16))
        .unwrap();
    assert_ne!(small, large);

    covers.store(small.clone(), Some(protocol()));
    assert!(covers.protocol(&small).is_some());
    assert!(covers.protocol(&large).is_none());
}

#[test]
fn a_cover_is_claimed_once_while_in_flight_and_once_it_has_landed() {
    let mut covers = covers();
    let key = key("s1");
    assert!(covers.claim(&key));
    assert!(!covers.claim(&key), "a second claim is a second request");
    covers.store(key.clone(), Some(protocol()));
    assert!(!covers.claim(&key));
}

#[test]
fn an_image_the_server_does_not_have_is_not_asked_for_again() {
    let mut covers = covers();
    let key = key("s1");
    assert!(covers.claim(&key));
    covers.store(key.clone(), None);
    assert!(!covers.claim(&key));
    assert!(covers.protocol(&key).is_none());
}

#[test]
fn a_request_that_merely_failed_is_tried_again_next_time() {
    // A network blip must not cost the item its artwork for the session, the
    // way a genuinely missing image does.
    let mut covers = covers();
    let key = key("s1");
    assert!(covers.claim(&key));
    covers.release(&key);
    assert!(covers.claim(&key));
}

#[test]
fn the_cache_drops_its_oldest_covers_rather_than_growing_with_the_library() {
    let mut covers = covers();
    for index in 0..CACHE_CAPACITY + 8 {
        covers.store(key(&format!("s{index}")), Some(protocol()));
    }
    assert!(covers.protocol(&key("s0")).is_none());
    assert!(
        covers
            .protocol(&key(&format!("s{}", CACHE_CAPACITY + 7)))
            .is_some()
    );
}

#[test]
fn a_terminal_that_reports_no_pixel_size_is_left_alone() {
    // tmux and a plain xterm leave the fields at zero, and halfblocks are what
    // they draw with anyway.
    assert_eq!(cell_size(window(Size::new(80, 24), Size::new(0, 0))), None);
    assert_eq!(
        cell_size(window(Size::new(0, 0), Size::new(800, 480))),
        None
    );
    assert_eq!(
        cell_size(window(Size::new(80, 24), Size::new(800, 480))),
        Some(Size::new(10, 20))
    );
}

#[test]
fn a_display_of_another_scale_encodes_against_it_and_drops_the_old_grid() {
    // The columns and rows do not have to move: the same cell box on a 1.5x
    // display is half again as many pixels, and the cover cached for the old
    // grid is the wrong encoding rather than a stale one.
    let mut covers = covers();
    let box_ = Size::new(10, 8);
    let before = covers.key(&item("s1", Some("tag")), box_).unwrap();
    covers.store(before.clone(), Some(protocol()));

    covers.set_cell_size(Some(Size::new(15, 30)));
    assert_eq!(covers.font_size().width, 15);
    let after = covers.key(&item("s1", Some("tag")), box_).unwrap();
    assert_ne!(before, after);
    assert!(covers.protocol(&after).is_none());
    assert!(
        covers.protocol(&before).is_none(),
        "the old grid is dropped"
    );
}

#[test]
fn the_terminals_padding_is_not_mistaken_for_a_display_scale() {
    // The window is a few pixels wider than the cells it holds, so a
    // measurement is a hair over — and re-encoding every cover for that would
    // be a round trip each time the window moved by a pixel.
    let mut covers = covers();
    let key = covers
        .key(&item("s1", Some("tag")), Size::new(10, 8))
        .unwrap();
    covers.store(key.clone(), Some(protocol()));

    covers.set_cell_size(cell_size(window(Size::new(80, 24), Size::new(816, 488))));
    assert_eq!(covers.font_size().width, 10);
    assert!(covers.protocol(&key).is_some());
}

#[test]
fn a_librarys_banner_is_measured_by_the_server_not_guessed_from_its_kind() {
    // `/UserViews` answers with the ratio; a box sized for the 2:3 poster a
    // CollectionFolder would otherwise get leaves the rail's text stranded
    // half a screen below the picture.
    let library = Item::deserialize(serde_json::json!({
        "Id": "l1", "Name": "Movies", "Type": "CollectionFolder",
        "PrimaryImageAspectRatio": 1.777_777_777_777_777_7
    }))
    .unwrap();
    assert!((primary_aspect(&library) - 16.0 / 9.0).abs() < 0.001);

    // `/Items` leaves it out, so the kind still has to answer for these.
    assert!((primary_aspect(&item("s1", None)) - 2.0 / 3.0).abs() < 0.001);
    let episode = Item::deserialize(serde_json::json!({
        "Id": "e1", "Type": "Episode", "PrimaryImageAspectRatio": null
    }))
    .unwrap();
    assert!((primary_aspect(&episode) - 16.0 / 9.0).abs() < 0.001);
}
