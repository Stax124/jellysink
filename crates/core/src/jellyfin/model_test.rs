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

/// Trimmed from a real `/Items` response, which is where the percentage that
/// answers for a folder comes from — the ticks are zero on a series however
/// much of it has been watched.
fn part_watched_series() -> serde_json::Value {
    json!({
        "Id": "ae3555401a49fc006fd79fdad5b1966b",
        "Name": "That Time I Got Reincarnated as a Slime",
        "Type": "Series",
        "ProductionYear": 2018,
        "CommunityRating": 8.0,
        "RunTimeTicks": 14400000000i64,
        "UserData": {
            "PlayedPercentage": 55.33980582524271,
            "UnplayedItemCount": 46,
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false
        }
    })
}

#[test]
fn a_series_takes_its_progress_from_the_percentage_because_its_ticks_are_zero() {
    let item = Item::deserialize(part_watched_series()).unwrap();
    assert_eq!(item.resume_ticks(), 0);
    assert!((item.watched_fraction().unwrap() - 0.5534).abs() < 0.001);
    assert_eq!(item.unplayed_count(), Some(46));
}

#[test]
fn a_finished_series_has_nothing_left_and_an_untouched_one_shows_no_bar() {
    let mut finished = part_watched_series();
    finished["UserData"]["PlayedPercentage"] = json!(100.0);
    finished["UserData"]["UnplayedItemCount"] = json!(0);
    finished["UserData"]["Played"] = json!(true);
    let finished = Item::deserialize(finished).unwrap();
    assert_eq!(finished.watched_fraction(), Some(1.0));
    assert_eq!(finished.unplayed_count(), None);

    let mut untouched = part_watched_series();
    untouched["UserData"]["PlayedPercentage"] = json!(0.0);
    let untouched = Item::deserialize(untouched).unwrap();
    assert_eq!(untouched.watched_fraction(), None);
}

#[test]
fn a_series_the_server_sent_no_percentage_for_shows_no_bar_rather_than_its_runtime() {
    // `RecursiveItemCount` unasked: the ticks are the nominal episode length,
    // and dividing a zero position by them would be meaningless either way.
    let mut raw = part_watched_series();
    raw["UserData"]["PlayedPercentage"] = json!(null);
    let item = Item::deserialize(raw).unwrap();
    assert_eq!(item.watched_fraction(), None);
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
    assert_eq!(item.unplayed_count(), None);
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
