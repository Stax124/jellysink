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
    CoverKey::primary(&item(id, Some("tag")), Size::new(2, 2)).unwrap()
}

fn covers() -> Covers {
    Covers::new(Picker::halfblocks(), 1.0)
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
    assert!(CoverKey::primary(&item("s1", None), Size::new(4, 4)).is_none());
    assert!(CoverKey::primary(&item("s1", Some("tag")), Size::new(4, 4)).is_some());
}

#[test]
fn a_resized_terminal_asks_for_a_new_encoding_instead_of_stretching_the_old() {
    let small = CoverKey::primary(&item("s1", Some("tag")), Size::new(10, 8)).unwrap();
    let large = CoverKey::primary(&item("s1", Some("tag")), Size::new(20, 16)).unwrap();
    assert_ne!(small, large);

    let mut covers = covers();
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
fn a_hidpi_scale_buys_pixels_without_moving_the_cell_box() {
    // The box the grid reserved is unchanged; only the image inside it grows,
    // because kitty measures the placement against the real pixel grid.
    let box_ = Size::new(32, 24);
    assert_eq!(encoded_size(box_, 1.0), box_);
    assert_eq!(encoded_size(box_, 2.0), Size::new(64, 48));
    assert_eq!(encoded_size(box_, 1.5), Size::new(48, 36));
}

#[test]
fn halfblocks_ignore_the_scale_because_an_over_encoded_one_is_cropped() {
    let covers = Covers::new(Picker::halfblocks(), 2.0);
    assert_eq!(covers.scale(), 1.0);
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
