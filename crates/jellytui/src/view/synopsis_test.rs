use super::*;
use crate::test_support::item;
use serde_json::json;

#[test]
fn the_band_names_a_favourite_and_when_it_was_last_watched() {
    // Trimmed from a real season listing, favourited.
    let episode = item(json!({
        "Id": "618101cf", "Name": "The Happy Roswaal Mansion Family", "Type": "Episode",
        "IndexNumber": 4, "ParentIndexNumber": 1,
        "RunTimeTicks": 31_001_000_000i64, "CommunityRating": 8.9,
        "UserData": {
            "PlayedPercentage": 61.57452985387568,
            "PlaybackPositionTicks": 19_088_720_000i64,
            "PlayCount": 5,
            "IsFavorite": true,
            "LastPlayedDate": "2026-09-29T22:40:02.6005731Z",
            "Played": false
        }
    }));
    assert_eq!(meta(&episode), "52 min · ★ 8.9 · ♥ · watched 29 Sep 2026");
}

#[test]
fn a_timestamp_that_is_not_a_date_is_left_out_rather_than_misread() {
    assert_eq!(
        calendar_day("2026-01-05T00:00:00Z").as_deref(),
        Some("5 Jan 2026")
    );
    assert_eq!(calendar_day("2026-13-05T00:00:00Z"), None);
    assert_eq!(calendar_day("yesterday"), None);
}
