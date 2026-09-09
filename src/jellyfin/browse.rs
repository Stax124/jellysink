//! Library browsing: the listing endpoints a frontend needs to walk a library
//! down to a playable item.

use super::auth::Api;
use super::encode_query_value;
use color_eyre::eyre::{Result, WrapErr};
use serde_json::Value;

/// Requested on every listing, so a row can be rendered without a second
/// round trip. `UserData` is not here: the server returns it unasked.
const ITEM_FIELDS: &str = "Overview,ProductionYear";

/// Jellyfin pages `/Shows/{id}/Episodes` without this; 500 covers a long
/// running series in one request.
pub(crate) const EPISODE_LIMIT: u32 = 500;

/// The `/Items` parameters that vary between screens. Private fields with
/// constructors instead of a literal, because `search` and `in_folder` differ
/// in `Recursive` and getting that wrong silently returns the whole library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ItemQuery {
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
    pub(crate) fn in_folder(parent_id: &str) -> Self {
        Self {
            parent_id: Some(parent_id.to_string()),
            sort_by: Some("SortName".to_string()),
            ..Self::default()
        }
    }

    pub(crate) fn search(term: &str) -> Self {
        Self {
            search_term: Some(term.to_string()),
            recursive: true,
            ..Self::default()
        }
    }

    pub(crate) fn with_types(mut self, include_item_types: &str) -> Self {
        self.include_item_types = Some(include_item_types.to_string());
        self
    }

    pub(crate) fn page(mut self, start_index: u32, limit: u32) -> Self {
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

impl Api {
    pub(crate) async fn user_views(&self) -> Result<Value> {
        let path = format!("/UserViews?userId={}", encode_query_value(&self.user_id));
        self.get_json(&path).await
    }

    pub(crate) async fn items(&self, query: &ItemQuery) -> Result<Value> {
        let path = format!("/Items?{}", query.to_query(&self.user_id));
        tracing::debug!(path, "GET items");
        self.get_json(&path).await
    }

    pub(crate) async fn seasons(&self, series_id: &str) -> Result<Value> {
        let path = format!(
            "/Shows/{series_id}/Seasons?userId={}",
            encode_query_value(&self.user_id)
        );
        self.get_json(&path).await
    }

    pub(crate) async fn episodes(
        &self,
        series_id: &str,
        season_id: Option<&str>,
        limit: u32,
    ) -> Result<Value> {
        let mut path = format!(
            "/Shows/{series_id}/Episodes?userId={}&Limit={limit}",
            encode_query_value(&self.user_id)
        );
        if let Some(season_id) = season_id {
            path.push_str("&seasonId=");
            path.push_str(&encode_query_value(season_id));
        }
        tracing::debug!(path, "GET episodes");
        self.get_json(&path).await
    }

    /// The whole series in aired order. No `StartItemId`: it is a forward-only
    /// `SkipWhile`, so the caller splits the listing itself.
    pub(crate) async fn episodes_all(&self, series_id: &str) -> Result<Value> {
        self.episodes(series_id, None, EPISODE_LIMIT).await
    }

    pub(crate) async fn next_up(&self, limit: u32) -> Result<Value> {
        let path = format!(
            "/Shows/NextUp?userId={}&Limit={limit}",
            encode_query_value(&self.user_id)
        );
        self.get_json(&path).await
    }

    /// Continue Watching. Falls back like [`Api::get_item`]: the modern path
    /// only exists from Jellyfin 10.9. The first error rides along on the
    /// second instead of being logged — jellytui installs no tracing
    /// subscriber, so a debug line here reaches nobody and the legacy 404
    /// would be all the user ever saw of, say, an expired token.
    pub(crate) async fn resume(&self, limit: u32) -> Result<Value> {
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
}

#[cfg(test)]
#[path = "browse_test.rs"]
mod tests;
