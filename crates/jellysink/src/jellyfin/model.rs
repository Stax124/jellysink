//! The item and session shapes a frontend reads back.
//!
//! Deliberately narrow: only the fields something renders or acts on. The
//! playback path still works in `serde_json::Value`, because it forwards most
//! of what it receives rather than displaying it.

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct Item {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    #[serde(rename = "Type")]
    pub(crate) kind: Option<String>,
    pub(crate) collection_type: Option<String>,
    pub(crate) series_id: Option<String>,
    pub(crate) season_id: Option<String>,
    pub(crate) series_name: Option<String>,
    pub(crate) index_number: Option<i64>,
    pub(crate) parent_index_number: Option<i64>,
    pub(crate) production_year: Option<i64>,
    pub(crate) run_time_ticks: Option<i64>,
    pub(crate) is_folder: bool,
    pub(crate) user_data: Option<UserData>,
    pub(crate) overview: Option<String>,
    pub(crate) community_rating: Option<f64>,
    pub(crate) official_rating: Option<String>,
    pub(crate) image_tags: ImageTags,
}

/// Jellyfin sends these unasked, keyed by image kind. Only the one the
/// frontend draws is modelled.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct ImageTags {
    pub(crate) primary: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct UserData {
    pub(crate) played: bool,
    pub(crate) playback_position_ticks: i64,
    pub(crate) played_percentage: Option<f64>,
}

impl Item {
    pub(crate) fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or_default()
    }

    /// The `is_folder` fallback is guarded: Jellyfin marks some playable
    /// items as folders (a movie that is really a disc directory).
    pub(crate) fn is_container(&self) -> bool {
        matches!(
            self.kind(),
            "Series" | "Season" | "CollectionFolder" | "Folder" | "BoxSet" | "UserView"
        ) || (self.is_folder && !self.is_playable())
    }

    pub(crate) fn is_playable(&self) -> bool {
        matches!(self.kind(), "Episode" | "Movie" | "Video")
    }

    /// Where a resume row should start. `0` means "from the beginning", which
    /// is also what the server wants when nothing was watched.
    pub(crate) fn resume_ticks(&self) -> i64 {
        self.user_data
            .as_ref()
            .map_or(0, |user_data| user_data.playback_position_ticks)
    }

    pub(crate) fn primary_image_tag(&self) -> Option<&str> {
        self.image_tags.primary.as_deref()
    }

    pub(crate) fn played(&self) -> bool {
        self.user_data.as_ref().is_some_and(|data| data.played)
    }

    /// How far in, 0.0..=1.0, for the progress bar on a partly watched row.
    pub(crate) fn watched_fraction(&self) -> Option<f64> {
        let ticks = self.run_time_ticks.filter(|ticks| *ticks > 0)?;
        let position = self.resume_ticks();
        (position > 0).then(|| (position as f64 / ticks as f64).clamp(0.0, 1.0))
    }

    /// The row label. Episodes lead with their number so a season list lines
    /// up; everything else is its name, with a movie's year for disambiguation.
    pub(crate) fn label(&self) -> String {
        let name = self.name.as_deref().unwrap_or("Untitled");
        match self.kind() {
            "Episode" => match (self.parent_index_number, self.index_number) {
                (Some(season), Some(episode)) => format!("S{season:02}E{episode:02}  {name}"),
                (None, Some(episode)) => format!("E{episode:02}  {name}"),
                _ => name.to_string(),
            },
            "Movie" => match self.production_year {
                Some(year) => format!("{name} ({year})"),
                None => name.to_string(),
            },
            _ => name.to_string(),
        }
    }

    pub(crate) fn sublabel(&self) -> Option<String> {
        match self.kind() {
            "Episode" => self.series_name.clone(),
            "Series" => self.production_year.map(|year| year.to_string()),
            _ => None,
        }
    }
}

/// One entry of `GET /Sessions`. Only the id: the response also embeds the
/// whole play queue as full items, which is why nothing else here reads it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct Session {
    pub(crate) id: String,
}

/// The `{ "Items": [...] }` envelope every listing endpoint returns.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct ItemList {
    pub(crate) items: Vec<Item>,
}

#[cfg(test)]
#[path = "model_test.rs"]
mod tests;
