//! Tests against a live Jellyfin: the one `testserver/start.sh` starts, holding
//! the library `testserver/episodes.tsv` describes.

use super::auth::{Api, is_auth_expired, login};
use super::browse::ItemQuery;
use super::model::{Item, ItemList};
use crate::config::Credentials;
use crate::error::UsageError;
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicU32, Ordering};

const USERNAME: &str = "jellysink";
const PASSWORD: &str = "jellysink";
const SERIES: &str = "That Time I Got Reincarnated as a Slime";

fn server() -> String {
    let port = std::env::var("JELLYSINK_TEST_PORT").unwrap_or_else(|_| "8096".to_string());
    format!("http://127.0.0.1:{port}")
}

/// One per login, so tests running in parallel never share a server session.
fn device_id() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "jellysink-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

async fn test_credentials() -> Credentials {
    crate::install_crypto_provider();
    let server = server();
    login(&server, USERNAME, PASSWORD, &device_id())
        .await
        .unwrap_or_else(|e| {
            panic!(
                "no Jellyfin test server at {server} ({e:#}); start one with `testserver/start.sh`"
            )
        })
}

async fn test_api() -> Api {
    Api::from_credentials(&test_credentials().await).unwrap()
}

fn items(listing: Value) -> Vec<Item> {
    ItemList::deserialize(listing).unwrap().items
}

async fn series(api: &Api) -> Item {
    let query = ItemQuery::search("Slime").with_types("Series");
    items(api.items(&query).await.unwrap())
        .into_iter()
        .find(|item| item.name.as_deref() == Some(SERIES))
        .expect("the test library's series")
}

#[tokio::test]
async fn a_wrong_password_is_a_usage_error() {
    crate::install_crypto_provider();
    let err = login(&server(), USERNAME, "wrong", &device_id())
        .await
        .unwrap_err();
    assert!(err.downcast_ref::<UsageError>().is_some(), "{err:#}");
}

#[tokio::test]
async fn a_rejected_token_is_auth_expired() {
    let creds = Credentials {
        access_token: "not-a-token".to_string(),
        ..test_credentials().await
    };
    let err = Api::from_credentials(&creds)
        .unwrap()
        .user_views()
        .await
        .unwrap_err();
    assert!(is_auth_expired(&err), "{err:#}");
}

#[tokio::test]
async fn the_library_browses_the_way_jellytui_walks_it() {
    let api = test_api().await;

    let views = items(api.user_views().await.unwrap());
    let shows = views
        .iter()
        .find(|view| view.collection_type.as_deref() == Some("tvshows"))
        .expect("a TV library view");
    let in_view = items(api.items(&ItemQuery::in_folder(&shows.id)).await.unwrap());
    let series = in_view
        .into_iter()
        .find(|item| item.name.as_deref() == Some(SERIES))
        .expect("the series in its library view");

    let seasons = items(api.seasons(&series.id).await.unwrap());
    let shape: Vec<_> = seasons
        .iter()
        .map(|season| (season.index_number, season.recursive_item_count))
        .collect();
    assert_eq!(
        shape,
        [0, 1, 2, 3, 4].map(|index| (Some(index), Some(if index == 0 { 10 } else { 24 })))
    );

    let season_four = items(api.episodes(&series.id, &seasons[4].id).await.unwrap());
    assert_eq!(season_four.len(), 24);
    assert_eq!(
        season_four.last().unwrap().name.as_deref(),
        Some("The Hero Awakens")
    );
    assert_eq!(
        items(api.episodes_all(&series.id).await.unwrap()).len(),
        106
    );

    let item = Item::deserialize(api.get_item(&series.id).await.unwrap()).unwrap();
    assert_eq!(item.production_year, Some(2018));
    let tag = item.primary_image_tag().expect("the series poster");
    let poster = api.primary_image(&series.id, tag, 64, 64).await.unwrap();
    assert!(poster.is_some_and(|bytes| !bytes.is_empty()));
}

#[tokio::test]
async fn the_home_rows_answer() {
    let api = test_api().await;
    items(api.next_up(20).await.unwrap());
    items(api.resume(20).await.unwrap());
}

#[tokio::test]
async fn set_played_round_trips() {
    let api = test_api().await;
    let series = series(&api).await;
    let specials = items(api.seasons(&series.id).await.unwrap())
        .into_iter()
        .find(|season| season.index_number == Some(0))
        .expect("the Specials season");
    let episode = items(api.episodes(&series.id, &specials.id).await.unwrap())
        .into_iter()
        .next()
        .expect("a special");

    for played in [true, false] {
        api.set_played(&episode.id, played).await.unwrap();
        let item = Item::deserialize(api.get_item(&episode.id).await.unwrap()).unwrap();
        assert_eq!(item.played(), played);
    }
}
