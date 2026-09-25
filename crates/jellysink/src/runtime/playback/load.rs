//! Getting a prepared play into mpv: the stream URL, where the access token
//! rides, and spawning mpv when there is none.

use crate::media::{PreparedPlay, TrackKind};
use crate::mpv::MpvSession;
use crate::runtime::state::Runtime;
use color_eyre::eyre::WrapErr;
use jellysink_core::config::{Config, MpvArgs, Paths};
use jellysink_core::jellyfin::auth::Api;

impl Runtime {
    /// `loadfile ... replace` into the running mpv, spawning one first if
    /// there is none.
    pub(super) async fn load_current(&mut self, prepared: &PreparedPlay) -> color_eyre::Result<()> {
        let mpv = match self.mpv.take() {
            Some(mpv) => mpv,
            None => spawn_mpv(&self.config, &self.paths).await?,
        };
        let mpv = self.mpv.insert(mpv);
        self.transitioning = true;
        // Items mpv autoplays skip this foreign-host check and the header follows
        // mpv into them, so with a next item queued the token rides the URL.
        let auth = apply_auth(&self.api, mpv, prepared, self.window.has_next()).await?;
        mpv.loadfile(&auth.url, Some(prepared.title.as_str()))
            .await?;
        self.mpv_auth_header_set = auth.header_set;
        if let Err(e) = mpv.set_volume(self.volume).await {
            tracing::warn!("could not restore volume in mpv: {e:#}");
        }
        if let Err(e) = mpv.set_mute(self.muted).await {
            tracing::warn!("could not restore mute in mpv: {e:#}");
        }
        if let Err(e) = mpv.set_pause(false).await {
            tracing::warn!("could not unpause mpv: {e:#}");
        }
        Ok(())
    }
}

async fn spawn_mpv(config: &Config, paths: &Paths) -> color_eyre::Result<MpvSession> {
    // Re-read mpv_args on every spawn so edits apply to the next play
    // without restarting the daemon.
    let mpv_args = MpvArgs::load(paths)
        .inspect_err(|e| {
            tracing::warn!("mpv_args unreadable; spawning without extra args: {e:#}");
        })
        .unwrap_or_default();
    let mut mpv = MpvSession::spawn(&config.mpv_path, &mpv_args.0, paths.mpv_socket()).await?;
    mpv.set_keep_open().await?;
    for kind in [TrackKind::Subtitle, TrackKind::Audio] {
        if let Err(e) = mpv.observe_track(kind).await {
            tracing::warn!(
                kind = kind.as_str(),
                "cannot observe mpv's track ({e:#}); a track picked in the mpv window \
                 will not be remembered or reported"
            );
        }
    }
    tracing::info!("mpv spawned");
    Ok(mpv)
}

fn stream_url_with_token(api: &Api, prepared: &PreparedPlay) -> String {
    if prepared.url.contains("ApiKey=") {
        prepared.url.clone()
    } else {
        jellysink_core::jellyfin::url::direct_stream_url(
            &api.server,
            &prepared.item_id,
            &prepared.media_source_id,
            prepared.live_stream_id.as_deref(),
            Some(&api.token),
        )
    }
}

/// The URL to hand mpv, plus whether mpv now carries the Authorization header.
/// The header is global, so it keeps the token out of watch_later files.
struct AppliedAuth {
    url: String,
    header_set: bool,
}

async fn apply_auth(
    api: &Api,
    mpv: &mut MpvSession,
    prepared: &PreparedPlay,
    force_url_token: bool,
) -> color_eyre::Result<AppliedAuth> {
    if !force_url_token && prepared.uses_auth_header {
        if let Err(e) = mpv.apply_auth_header(&api.mpv_auth_header_field()).await {
            tracing::warn!("could not set mpv auth header ({e:#}); putting ApiKey on the URL");
            return Ok(AppliedAuth {
                url: stream_url_with_token(api, prepared),
                header_set: false,
            });
        }
        return Ok(AppliedAuth {
            url: prepared.url.clone(),
            header_set: true,
        });
    }
    // A header left from an earlier item would reach this one's subtitle host.
    mpv.clear_auth_header()
        .await
        .wrap_err("clearing mpv's Authorization header")?;
    Ok(AppliedAuth {
        url: stream_url_with_token(api, prepared),
        header_set: false,
    })
}
