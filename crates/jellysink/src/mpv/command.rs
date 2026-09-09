//! The typed commands we send mpv, and the argument shapes it insists on.

use super::MpvSession;
use super::event::{
    AUDIO_TRACK_OBSERVER_ID, AUDIO_TRACK_PROPERTY, SUBTITLE_TRACK_OBSERVER_ID,
    SUBTITLE_TRACK_PROPERTY, SelectedTrack, selected_track_from_property,
};
use super::ipc::json_as_seconds;
use color_eyre::eyre::{WrapErr, eyre};
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// M3U with one `#EXTINF` entry per `(title, url)`. The only way to give
/// unloaded playlist entries a title.
pub(crate) fn playlist_m3u<I, T, U>(entries: I) -> String
where
    I: IntoIterator<Item = (T, U)>,
    T: AsRef<str>,
    U: AsRef<str>,
{
    let mut body = String::from("#EXTM3U\n");
    for (title, url) in entries {
        let title = title.as_ref().replace(['\r', '\n'], " ");
        body.push_str("#EXTINF:-1,");
        body.push_str(&title);
        body.push('\n');
        body.push_str(url.as_ref());
        body.push('\n');
    }
    body
}

pub(crate) fn loadlist_append_args(path: &str) -> [Value; 3] {
    [json!("loadlist"), json!(path), json!("append")]
}

/// `insert-at` and the index must stay separate arguments; `"insert-at0"` is
/// `invalid parameter`.
pub(crate) fn loadlist_insert_at_args(path: &str, index: usize) -> [Value; 4] {
    [
        json!("loadlist"),
        json!(path),
        json!("insert-at"),
        json!(index),
    ]
}

/// `yes` auto-plays the rest of the playlist and emits the `end-file` autoplay
/// keys off; `always` unloads nothing and never emits it.
pub(crate) const KEEP_OPEN: &str = "yes";

/// Highest mpv subtitle track id (`sid`) in a track-list, typically after `sub-add`.
pub(crate) fn max_subtitle_track_id_from_track_list(list: &Value) -> i64 {
    let mut max = 0i64;
    if let Some(arr) = list.as_array() {
        for t in arr {
            if t.get("type").and_then(Value::as_str) == Some("sub")
                && let Some(id) = t.get("id").and_then(Value::as_i64)
            {
                max = max.max(id);
            }
        }
    }
    max
}

impl MpvSession {
    pub(crate) async fn loadfile(
        &mut self,
        url: &str,
        title: Option<&str>,
    ) -> color_eyre::Result<()> {
        // Since mpv 0.38 loadfile's 4th argument is an insert index, not an
        // options map, so force-media-title has to go through a property.
        if let Some(title) = title {
            let _ = self.set_property("force-media-title", json!(title)).await;
        }
        self.command(vec![json!("loadfile"), json!(url), json!("replace")])
            .await?;
        Ok(())
    }

    /// Appends every entry in one `loadlist`. Titles come from `#EXTINF`.
    pub(crate) async fn loadlist_append(
        &mut self,
        entries: &[(&str, &str)],
    ) -> color_eyre::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.socket.with_file_name("append.m3u");
        self.loadlist(&path, playlist_m3u(entries.iter().copied()), None)
            .await
    }

    /// Splices every entry in at `index` in one `loadlist`. Playback is
    /// unaffected; mpv shifts `playlist-pos`.
    pub(crate) async fn loadlist_insert_at(
        &mut self,
        entries: &[(&str, &str)],
        index: usize,
    ) -> color_eyre::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.socket.with_file_name("insert.m3u");
        self.loadlist(&path, playlist_m3u(entries.iter().copied()), Some(index))
            .await
    }

    /// Writes an M3U next to the IPC socket, loads it, then removes it. The
    /// file is what carries each entry's title.
    async fn loadlist(
        &mut self,
        path: &Path,
        body: String,
        index: Option<usize>,
    ) -> color_eyre::Result<()> {
        write_private(path, &body).await?;
        let args: Vec<Value> = match index {
            Some(i) => loadlist_insert_at_args(&path.to_string_lossy(), i).to_vec(),
            None => loadlist_append_args(&path.to_string_lossy()).to_vec(),
        };
        let result = self.command(args).await;
        let _ = tokio::fs::remove_file(path).await;
        result?;
        Ok(())
    }

    pub(crate) async fn playlist_next(&mut self) -> color_eyre::Result<()> {
        self.command(vec![json!("playlist-next"), json!("force")])
            .await?;
        Ok(())
    }

    pub(crate) async fn playlist_prev(&mut self) -> color_eyre::Result<()> {
        self.command(vec![json!("playlist-prev"), json!("force")])
            .await?;
        Ok(())
    }

    /// mpv reports `-1` while idle; that is a real answer, not a failure.
    pub(crate) async fn playlist_pos(&mut self) -> color_eyre::Result<i64> {
        self.get_i64("playlist-pos").await
    }

    pub(crate) async fn playlist_count(&mut self) -> color_eyre::Result<i64> {
        self.get_i64("playlist-count").await
    }

    pub(crate) async fn set_keep_open(&mut self) -> color_eyre::Result<()> {
        self.set_property("keep-open", json!(KEEP_OPEN)).await
    }

    pub(crate) async fn sub_add(&mut self, url: &str) -> color_eyre::Result<()> {
        self.command(vec![json!("sub-add"), json!(url)]).await?;
        Ok(())
    }

    pub(crate) async fn apply_auth_header(&mut self, header_field: &str) -> color_eyre::Result<()> {
        self.set_property("http-header-fields", json!([header_field]))
            .await
    }

    pub(crate) async fn clear_auth_header(&mut self) -> color_eyre::Result<()> {
        self.set_property("http-header-fields", json!([])).await
    }

    pub(crate) async fn pause(&mut self) -> color_eyre::Result<()> {
        self.set_property("pause", json!(true)).await
    }

    pub(crate) async fn unpause(&mut self) -> color_eyre::Result<()> {
        self.set_property("pause", json!(false)).await
    }

    pub(crate) async fn toggle_pause(&mut self) -> color_eyre::Result<()> {
        let paused = self.get_bool("pause").await?;
        self.set_property("pause", json!(!paused)).await
    }

    pub(crate) async fn seek_absolute(&mut self, seconds: f64) -> color_eyre::Result<()> {
        self.command(vec![json!("seek"), json!(seconds), json!("absolute")])
            .await?;
        Ok(())
    }

    pub(crate) async fn set_volume(&mut self, volume: i64) -> color_eyre::Result<()> {
        self.set_property("volume", json!(volume.clamp(0, 100)))
            .await
    }

    pub(crate) async fn add_volume(&mut self, delta: i64) -> color_eyre::Result<i64> {
        let cur = self.get_f64("volume").await? as i64;
        let next = (cur + delta).clamp(0, 100);
        self.set_volume(next).await?;
        Ok(next)
    }

    pub(crate) async fn set_mute(&mut self, mute: bool) -> color_eyre::Result<()> {
        self.set_property("mute", json!(mute)).await
    }

    /// `None` or a negative id means `aid=no`, where `cycle audio` lands after
    /// the last track.
    pub(crate) async fn set_audio_track_id(
        &mut self,
        audio_track_id: Option<i64>,
    ) -> color_eyre::Result<()> {
        match audio_track_id {
            Some(id) if id >= 0 => self.set_property(AUDIO_TRACK_PROPERTY, json!(id)).await,
            _ => self.set_property(AUDIO_TRACK_PROPERTY, json!("no")).await,
        }
    }

    pub(crate) async fn audio_track(&mut self) -> color_eyre::Result<SelectedTrack> {
        Ok(selected_track_from_property(
            &self.get_property(AUDIO_TRACK_PROPERTY).await?,
        ))
    }

    /// So a track picked in the mpv window, not a Jellyfin client, is noticed.
    pub(crate) async fn observe_audio_track(&mut self) -> color_eyre::Result<()> {
        self.command(vec![
            json!("observe_property"),
            json!(AUDIO_TRACK_OBSERVER_ID),
            json!(AUDIO_TRACK_PROPERTY),
        ])
        .await?;
        Ok(())
    }

    pub(crate) async fn set_subtitle_track_id(
        &mut self,
        subtitle_track_id: Option<i64>,
    ) -> color_eyre::Result<()> {
        match subtitle_track_id {
            Some(id) if id >= 0 => self.set_property(SUBTITLE_TRACK_PROPERTY, json!(id)).await,
            _ => {
                self.set_property(SUBTITLE_TRACK_PROPERTY, json!("no"))
                    .await
            }
        }
    }

    pub(crate) async fn subtitle_track(&mut self) -> color_eyre::Result<SelectedTrack> {
        Ok(selected_track_from_property(
            &self.get_property(SUBTITLE_TRACK_PROPERTY).await?,
        ))
    }

    /// So a track picked in the mpv window, not a Jellyfin client, is noticed.
    pub(crate) async fn observe_subtitle_track(&mut self) -> color_eyre::Result<()> {
        self.command(vec![
            json!("observe_property"),
            json!(SUBTITLE_TRACK_OBSERVER_ID),
            json!(SUBTITLE_TRACK_PROPERTY),
        ])
        .await?;
        Ok(())
    }

    pub(crate) async fn max_subtitle_track_id(&mut self) -> color_eyre::Result<i64> {
        let list = self.get_property("track-list").await?;
        Ok(max_subtitle_track_id_from_track_list(&list))
    }

    pub(crate) async fn toggle_fullscreen(&mut self) -> color_eyre::Result<()> {
        let fs = self.get_bool("fullscreen").await?;
        self.set_property("fullscreen", json!(!fs)).await
    }

    pub(crate) async fn time_pos(&mut self) -> color_eyre::Result<f64> {
        let v = self.get_property("time-pos").await?;
        json_as_seconds(&v).ok_or_else(|| eyre!("time-pos was not a number"))
    }

    pub(crate) async fn paused(&mut self) -> color_eyre::Result<bool> {
        self.get_bool("pause").await
    }

    pub(crate) async fn volume(&mut self) -> color_eyre::Result<i64> {
        Ok(self.get_f64("volume").await? as i64)
    }

    pub(crate) async fn muted(&mut self) -> color_eyre::Result<bool> {
        self.get_bool("mute").await
    }
}

/// Creates the file 0600 rather than chmodding after: the M3U body can carry
/// `ApiKey=`, so it must never exist world-readable, not even briefly.
async fn write_private(path: &Path, body: &str) -> color_eyre::Result<()> {
    let _ = tokio::fs::remove_file(path).await;
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await
        .wrap_err_with(|| format!("creating {}", path.display()))?;
    f.write_all(body.as_bytes())
        .await
        .wrap_err_with(|| format!("writing {}", path.display()))?;
    // tokio's File does not flush on drop, and mpv reads the path back at once.
    f.flush()
        .await
        .wrap_err_with(|| format!("flushing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "command_test.rs"]
mod tests;
