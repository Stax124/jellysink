//! Library browsing: the listing endpoints a frontend needs to walk a library
//! down to a playable item.

use super::auth::Api;
use super::encode_query_value;
use color_eyre::eyre::{Result, WrapErr};
use serde_json::Value;

/// Requested on every listing, so a row renders without a second round trip.
/// `UserData` is not here: the server returns it unasked, but leaves
/// `PlayedPercentage` null without `RecursiveItemCount`.
const ITEM_FIELDS: &str = "Overview,ProductionYear,RecursiveItemCount,ChildCount,Genres";

/// Jellyfin pages `/Shows/{id}/Episodes` without this; 500 covers a long
/// running series in one request.
pub const EPISODE_LIMIT: u32 = 500;

/// The `/Items` parameters that vary between screens. Constructors rather than
/// a literal: getting `Recursive` wrong silently returns the whole library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemQuery {
    parent_id: Option<String>,
    search_term: Option<String>,
    include_item_types: Option<String>,
    recursive: bool,
    sort_by: Option<String>,
    start_index: Option<u32>,
    limit: Option<u32>,
}

impl ItemQuery {
    /// The direct children of one folder, in the server's sort order.
    pub fn in_folder(parent_id: &str) -> Self {
        Self {
            parent_id: Some(parent_id.to_string()),
            sort_by: Some("SortName".to_string()),
            ..Self::default()
        }
    }

    pub fn search(term: &str) -> Self {
        Self {
            search_term: Some(term.to_string()),
            recursive: true,
            ..Self::default()
        }
    }

    pub fn with_types(mut self, include_item_types: &str) -> Self {
        self.include_item_types = Some(include_item_types.to_string());
        self
    }

    pub fn page(mut self, start_index: u32, limit: u32) -> Self {
        self.start_index = Some(start_index);
        self.limit = Some(limit);
        self
    }

    pub(crate) fn to_query(&self, user_id: &str) -> String {
        let mut query = format!(
            "userId={}&Fields={ITEM_FIELDS}&Recursive={}",
            encode_query_value(user_id),
            self.recursive
        );
        let mut push = |key: &str, value: &str| {
            query.push('&');
            query.push_str(key);
            query.push('=');
            query.push_str(&encode_query_value(value));
        };
        if let Some(parent_id) = &self.parent_id {
            push("ParentId", parent_id);
        }
        if let Some(search_term) = &self.search_term {
            push("searchTerm", search_term);
        }
        if let Some(include_item_types) = &self.include_item_types {
            push("IncludeItemTypes", include_item_types);
        }
        if let Some(sort_by) = &self.sort_by {
            push("SortBy", sort_by);
        }
        if let Some(start_index) = self.start_index {
            push("StartIndex", &start_index.to_string());
        }
        if let Some(limit) = self.limit {
            push("Limit", &limit.to_string());
        }
        query
    }
}

/// Sizes are rounded up to this so the server's resize cache is hit rather
/// than re-encoded per terminal; `specs/tui.md` has the why.
const IMAGE_BUCKET_PIXELS: u32 = 64;

/// Left off, the server answers far above 90 — a grid cover measured 33 KB
/// unasked against 12 KB here. 100 is a cliff: WebP turns near-lossless.
const IMAGE_QUALITY: u32 = 85;

pub fn bucket_pixels(pixels: u32) -> u32 {
    pixels.max(1).div_ceil(IMAGE_BUCKET_PIXELS) * IMAGE_BUCKET_PIXELS
}

fn primary_image_path(item_id: &str, image_tag: &str, max_width: u32, max_height: u32) -> String {
    format!(
        "/Items/{item_id}/Images/Primary?maxWidth={}&maxHeight={}&format=Webp&quality={IMAGE_QUALITY}&tag={}",
        bucket_pixels(max_width),
        bucket_pixels(max_height),
        encode_query_value(image_tag)
    )
}

impl Api {
    pub async fn user_views(&self) -> Result<Value> {
        let path = format!("/UserViews?userId={}", encode_query_value(&self.user_id));
        self.get_json(&path).await
    }

    pub async fn items(&self, query: &ItemQuery) -> Result<Value> {
        let path = format!("/Items?{}", query.to_query(&self.user_id));
        tracing::debug!(path, "GET items");
        self.get_json(&path).await
    }

    /// `Fields` for the same reason [`ITEM_FIELDS`] carries it: a season's
    /// `PlayedPercentage` is null without it.
    pub async fn seasons(&self, series_id: &str) -> Result<Value> {
        let path = format!(
            "/Shows/{series_id}/Seasons?userId={}&Fields=RecursiveItemCount",
            encode_query_value(&self.user_id)
        );
        self.get_json(&path).await
    }

    /// One season, for a frontend that shows a synopsis. `Overview` costs a
    /// few hundred bytes an episode — worth it here, not in
    /// [`Api::episodes_all`].
    pub async fn episodes(
        &self,
        series_id: &str,
        season_id: Option<&str>,
        limit: u32,
    ) -> Result<Value> {
        self.episodes_listing(series_id, season_id, limit, true)
            .await
    }

    /// The whole series in aired order. No `StartItemId`: it is a forward-only
    /// `SkipWhile`, so the caller splits the listing itself.
    pub async fn episodes_all(&self, series_id: &str) -> Result<Value> {
        self.episodes_listing(series_id, None, EPISODE_LIMIT, false)
            .await
    }

    async fn episodes_listing(
        &self,
        series_id: &str,
        season_id: Option<&str>,
        limit: u32,
        with_overview: bool,
    ) -> Result<Value> {
        let mut path = format!(
            "/Shows/{series_id}/Episodes?userId={}&Limit={limit}",
            encode_query_value(&self.user_id)
        );
        if let Some(season_id) = season_id {
            path.push_str("&seasonId=");
            path.push_str(&encode_query_value(season_id));
        }
        if with_overview {
            path.push_str("&Fields=Overview");
        }
        tracing::debug!(path, "GET episodes");
        self.get_json(&path).await
    }

    /// The item's primary image, bounded to the box it is drawn in and rounded
    /// up to a bucket. `maxWidth`/`maxHeight` keep the aspect ratio;
    /// `fillWidth`/`fillHeight` do not.
    pub async fn primary_image(
        &self,
        item_id: &str,
        image_tag: &str,
        max_width: u32,
        max_height: u32,
    ) -> Result<Option<Vec<u8>>> {
        let path = primary_image_path(item_id, image_tag, max_width, max_height);
        let response = self.get(&path).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .wrap_err_with(|| format!("GET {path}"))?;
        Ok(Some(
            response
                .bytes()
                .await
                .wrap_err("reading image bytes")?
                .to_vec(),
        ))
    }

    pub async fn next_up(&self, limit: u32) -> Result<Value> {
        let path = format!(
            "/Shows/NextUp?userId={}&Limit={limit}",
            encode_query_value(&self.user_id)
        );
        self.get_json(&path).await
    }

    /// Continue Watching; the modern path only exists from Jellyfin 10.9. The
    /// first error rides along on the second rather than being logged, because
    /// jellytui installs no tracing subscriber.
    pub async fn resume(&self, limit: u32) -> Result<Value> {
        let user_id = encode_query_value(&self.user_id);
        let path = format!("/UserItems/Resume?userId={user_id}&Limit={limit}&MediaTypes=Video");
        let modern = match self.get_json(&path).await {
            Ok(listing) => return Ok(listing),
            Err(modern) => modern,
        };
        let legacy = format!("/Users/{user_id}/Items/Resume?Limit={limit}&MediaTypes=Video");
        self.get_json(&legacy)
            .await
            .wrap_err_with(|| format!("{modern:#}"))
    }
    pub async fn get_item(&self, item_id: &str) -> color_eyre::Result<Value> {
        let path = format!(
            "/Items/{item_id}?userId={}",
            encode_query_value(&self.user_id)
        );
        match self.get_json(&path).await {
            Ok(item) => Ok(item),
            Err(err) => {
                tracing::debug!(%err, path, "item lookup failed; trying legacy endpoint");
                let legacy = format!("/Users/{}/Items/{item_id}", self.user_id);
                self.get_json(&legacy).await
            }
        }
    }
}

#[cfg(test)]
#[path = "browse_test.rs"]
mod tests;
