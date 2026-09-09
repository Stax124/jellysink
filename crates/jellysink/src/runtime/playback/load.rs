//! Getting a prepared play into mpv: the stream URL, whether the access
//! token rides in the Authorization header or the query string, and spawning
//! mpv when there is none.

use crate::media::PreparedPlay;
use crate::mpv::MpvSession;
use crate::runtime::state::Runtime;
use crate::runtime::task::AbortOnDrop;
use color_eyre::eyre::eyre;
use jellysink_core::jellyfin::auth::Api;

impl Runtime {
    pub(super) async fn load_into_existing(
        &mut self,
        prep: &PreparedPlay,
        item_id: &str,
    ) -> color_eyre::Result<()> {
        let Some(mpv) = self.mpv.as_mut() else {
            return Err(eyre!("mpv missing during reuse"));
        };
        let auth = apply_auth(&self.api, mpv, prep, item_id, self.window.has_next()).await;
        mpv.loadfile(&auth.url, Some(prep.title.as_str())).await?;
        self.mpv_auth_header_set = auth.header_set;
        let _ = mpv.set_volume(self.volume).await;
        let _ = mpv.set_mute(self.muted).await;
        let _ = mpv.unpause().await;
        Ok(())
    }

    pub(super) async fn spawn_and_load(
        &mut self,
        prep: &PreparedPlay,
        item_id: &str,
    ) -> color_eyre::Result<()> {
        // Re-read mpv_args on every spawn so edits apply to the next play
        // without restarting the daemon.
        let mpv_args = jellysink_core::config::MpvArgs::load(&self.paths)
            .inspect_err(|e| {
                tracing::warn!("mpv_args unreadable; spawning without extra args: {e:#}");
            })
            .unwrap_or_default();
        let (mut mpv, events) =
            MpvSession::spawn(&self.config.mpv_path, &mpv_args.0, self.paths.mpv_socket()).await?;
        mpv.set_keep_open().await?;
        if let Err(e) = mpv.observe_subtitle_track().await {
            tracing::warn!(
                "cannot observe mpv's subtitle track ({e:#}); a track picked in the mpv \
                 window will not be remembered or reported"
            );
        }
        if let Err(e) = mpv.observe_audio_track().await {
            tracing::warn!(
                "cannot observe mpv's audio track ({e:#}); a track picked in the mpv \
                 window will not be remembered or reported"
            );
        }
        tracing::info!("mpv spawned");

        let auth = apply_auth(&self.api, &mut mpv, prep, item_id, self.window.has_next()).await;
        if let Err(e) = mpv.loadfile(&auth.url, Some(prep.title.as_str())).await {
            let _ = mpv.quit_and_wait().await;
            return Err(e);
        }
        self.mpv_auth_header_set = auth.header_set;
        let _ = mpv.set_volume(self.volume).await;
        let _ = mpv.set_mute(self.muted).await;

        self.mpv_gen = self.mpv_gen.wrapping_add(1);
        let generation = self.mpv_gen;
        let tx = self.mpv_tx.clone();
        // The assignment drops — and so aborts — the previous forwarder. The
        // events it already queued stay on the shared channel; `generation` is
        // how the main loop discards those.
        self.mpv_events = Some(AbortOnDrop(tokio::spawn(async move {
            let mut events = events;
            while let Some(ev) = events.recv().await {
                if tx.send((generation, ev)).is_err() {
                    break;
                }
            }
        })));
        self.mpv = Some(mpv);
        self.transitioning = true;
        Ok(())
    }
}

fn stream_url_with_token(api: &Api, item_id: &str, prep: &PreparedPlay) -> String {
    if prep.url.contains("ApiKey=") {
        prep.url.clone()
    } else {
        jellysink_core::jellyfin::url::direct_stream_url(
            &api.server,
            item_id,
            &prep.media_source_id,
            prep.live_stream_id.as_deref(),
            Some(&api.token),
        )
    }
}

/// The URL to hand mpv, plus whether mpv now carries the Authorization header.
/// The header is global, so it covers later playlist rows too and keeps the
/// token out of the URLs mpv writes to its watch_later files.
struct AppliedAuth {
    url: String,
    header_set: bool,
}

async fn apply_auth(
    api: &Api,
    mpv: &mut MpvSession,
    prep: &PreparedPlay,
    item_id: &str,
    force_url_token: bool,
) -> AppliedAuth {
    if !force_url_token && prep.uses_auth_header {
        match mpv.apply_auth_header(&api.mpv_auth_header_field()).await {
            Ok(()) => {
                return AppliedAuth {
                    url: prep.url.clone(),
                    header_set: true,
                };
            }
            Err(e) => {
                tracing::warn!("could not set mpv auth header ({e:#}); putting ApiKey on the URL");
                return AppliedAuth {
                    url: stream_url_with_token(api, item_id, prep),
                    header_set: false,
                };
            }
        }
    }
    let _ = mpv.clear_auth_header().await;
    AppliedAuth {
        url: stream_url_with_token(api, item_id, prep),
        header_set: false,
    }
}

/// Resume offsets only apply when positive; `None`/`0`/negative mean "start
/// from the beginning".
pub(crate) fn resume_seek_ticks(start_ticks: Option<i64>) -> Option<i64> {
    start_ticks.filter(|t| *t > 0)
}

#[cfg(test)]
#[path = "load_test.rs"]
mod tests;
