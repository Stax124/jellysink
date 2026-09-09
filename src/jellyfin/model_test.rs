use super::*;
use serde::Deserialize;
use serde_json::json;

/// Trimmed from a real `/UserItems/Resume` response.
fn resume_episode() -> serde_json::Value {
    json!({
        "Id": "76a89be6aa435bdbf92e7fd993af8c96",
        "Name": "Paradise, Once More",
        "Type": "Episode",
        "SeriesName": "That Time I Got Reincarnated as a Slime",
        "SeriesId": "aa11bb22",
        "IndexNumber": 3,
        "ParentIndexNumber": 2,
        "ProductionYear": 2021,
        "RunTimeTicks": 14220809999i64,
        "UserData": {
            "PlayedPercentage": 64.46236185311965,
            "PlaybackPositionTicks": 9167070000i64,
            "PlayCount": 1,
            "IsFavorite": false,
            "Played": false
        }
    })
}

#[test]
fn a_resume_episode_carries_its_position_and_reads_as_playable() {
    let item = Item::deserialize(resume_episode()).unwrap();
    assert_eq!(item.label(), "S02E03  Paradise, Once More");
    assert_eq!(
        item.sublabel().as_deref(),
        Some("That Time I Got Reincarnated as a Slime")
    );
    assert_eq!(item.resume_ticks(), 9_167_070_000);
    assert!(item.is_playable() && !item.is_container());
    assert!(!item.played());
    assert!((item.watched_fraction().unwrap() - 0.6446).abs() < 0.001);
}

#[test]
fn an_episode_without_a_season_number_still_gets_a_label() {
    let mut raw = resume_episode();
    raw["ParentIndexNumber"] = json!(null);
    let item = Item::deserialize(raw).unwrap();
    assert_eq!(item.label(), "E03  Paradise, Once More");
}

#[test]
fn an_item_the_server_sent_no_user_data_for_is_unwatched_not_a_panic() {
    let item = Item::deserialize(json!({
        "Id": "abc", "Name": "Dune", "Type": "Movie", "ProductionYear": 2021
    }))
    .unwrap();
    assert_eq!(item.label(), "Dune (2021)");
    assert_eq!(item.resume_ticks(), 0);
    assert!(!item.played());
    assert_eq!(item.watched_fraction(), None);
}

#[test]
fn a_library_folder_is_a_container_and_a_series_descends_rather_than_plays() {
    let library = Item::deserialize(json!({
        "Id": "f137", "Name": "Movies", "Type": "CollectionFolder", "IsFolder": true
    }))
    .unwrap();
    let series =
        Item::deserialize(json!({"Id": "b47d", "Name": "Bebop", "Type": "Series"})).unwrap();
    assert!(library.is_container() && !library.is_playable());
    assert!(series.is_container() && !series.is_playable());
}

#[test]
fn a_session_decodes_from_a_response_carrying_far_more_than_the_id() {
    let session = Session::deserialize(json!({
        "Id": "077175b0dc",
        "NowPlayingItem": resume_episode(),
        "NowPlayingQueueFullItems": [resume_episode(), resume_episode()],
        "PlayState": {"PositionTicks": 9167070000i64, "IsPaused": true}
    }))
    .unwrap();
    assert_eq!(session.id, "077175b0dc");
}

#[test]
fn a_listing_envelope_decodes_to_its_rows() {
    let list = ItemList::deserialize(json!({
        "Items": [resume_episode()], "TotalRecordCount": 1, "StartIndex": 0
    }))
    .unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].id, "76a89be6aa435bdbf92e7fd993af8c96");
}
