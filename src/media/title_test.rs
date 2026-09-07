use super::*;
use serde_json::json;
#[test]
fn episode_title_includes_series_and_numbers() {
    let item = json!({
        "Type": "Episode",
        "Name": "The One",
        "SeriesName": "Friends",
        "ParentIndexNumber": 1,
        "IndexNumber": 2
    });
    assert_eq!(display_title(&item), "Friends - s1e02 - The One");
}

#[test]
fn movie_title_includes_year() {
    let item = json!({"Type": "Movie", "Name": "Heat", "ProductionYear": 1995});
    assert_eq!(display_title(&item), "Heat (1995)");
}

#[test]
fn plain_name_when_metadata_is_thin() {
    let item = json!({"Name": "Home Video"});
    assert_eq!(display_title(&item), "Home Video");
}

#[test]
fn episode_titles_use_display_title() {
    let v = json!({
        "Items": [
            {
                "Id": "e1",
                "Type": "Episode",
                "Name": "Pilot",
                "SeriesName": "Show",
                "ParentIndexNumber": 1,
                "IndexNumber": 1
            },
            {
                "Id": "e2",
                "Type": "Episode",
                "Name": "Next",
                "SeriesName": "Show",
                "ParentIndexNumber": 1,
                "IndexNumber": 2
            }
        ]
    });
    let titles = episode_titles(&v);
    assert_eq!(titles.get("e1").unwrap(), "Show - s1e01 - Pilot");
    assert_eq!(titles.get("e2").unwrap(), "Show - s1e02 - Next");
}

#[test]
fn episode_titles_empty_on_malformed_payload() {
    assert!(episode_titles(&json!({})).is_empty());
    assert!(episode_titles(&json!({"Items": "nope"})).is_empty());
}
