//! The long-lived player state, and the command dispatch over it.

use super::task::AbortOnDrop;
use super::window::{EndFileAction, PlaylistWindow, end_file_action, ignore_stop_for_playlist};
use crate::cast::CastEvent;
use crate::config::{Config, Paths};
use crate::jellyfin::auth::Api;
use crate::media::{PlayRequest, PreparedPlay, TrackKind, TrackPreference};
use crate::mpv::{EndFileReason, MpvEvent, MpvSession, SelectedTrack};
use crate::report::Report;
use std::collections::HashMap;

/// Everything the runtime tracks about one kind of stream. Audio and subtitles
/// are handled by the same [`TrackKind`]-parameterised code, so they are two
/// values of one type rather than four separate fields.
#[derive(Debug, Default)]
pub(super) struct TrackState {
    /// mpv's selection as of the last time it was *ours* — the end of
    /// `configure_streams`, or an `apply_track`. A property change reporting
    /// anything else is the user picking a track in the mpv window.
    ///
    /// Per-mpv-session: `stop_playback` resets it.
    pub(super) settled: SelectedTrack,
    /// The track the user last picked by hand, re-applied to the next episode
    /// by identity rather than by index. In memory only, and deliberately never
    /// cleared by `start_current`, `adopt_playlist_pos` or `stop_playback`.
    pub(super) remembered: Option<TrackPreference>,
}

/// Where PlayNext / PlayLast put their items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enqueue {
    /// Immediately after the current item.
    Next,
    /// At the end of the queue.
    Last,
}

pub(super) struct Runtime {
    pub(super) api: Api,
    pub(super) config: Config,
    pub(super) paths: Paths,
    /// The queue plus the window of it mpv currently holds.
    pub(super) window: PlaylistWindow,
    pub(super) mpv: Option<MpvSession>,
    pub(super) mpv_tx: tokio::sync::mpsc::UnboundedSender<(u64, MpvEvent)>,
    /// Discriminates events still queued from a previous mpv session.
    pub(super) mpv_gen: u64,
    /// The task forwarding the current mpv session's events. Owned so a respawn
    /// or a stop does not leave it running.
    pub(super) mpv_events: Option<AbortOnDrop>,
    pub(super) current: Option<PreparedPlay>,
    pub(super) item_id: Option<String>,
    pub(super) volume: i64,
    pub(super) muted: bool,
    pub(super) paused: bool,
    pub(super) stopping: bool,
    pub(super) last_ticks: i64,
    /// Jellyfin subtitle stream index → mpv subtitle track id for `sub-add`ed files.
    pub(super) external_subtitle_track_ids: HashMap<i64, i64>,
    /// `aid`; see [`TrackState`].
    pub(super) audio: TrackState,
    /// `sid`; see [`TrackState`].
    pub(super) subtitle: TrackState,
    pub(super) report_tx: tokio::sync::mpsc::UnboundedSender<Report>,
    pub(super) transitioning: bool,
    /// When true, playlist stubs leave the token off their URLs — mpv persists
    /// playlist entries to its watch_later files.
    pub(super) mpv_auth_header_set: bool,
    pub(super) pending_start_ticks: Option<i64>,

    pub(super) prepared: HashMap<String, PreparedPlay>,
    /// Item id → display title, for the playlist fill; `PlaybackInfo` is
    /// fetched only once an item actually starts.
    pub(super) titles: HashMap<String, String>,
}

impl Runtime {
    pub(super) fn new(
        api: Api,
        config: Config,
        paths: Paths,
        mpv_tx: tokio::sync::mpsc::UnboundedSender<(u64, MpvEvent)>,
        report_tx: tokio::sync::mpsc::UnboundedSender<Report>,
    ) -> Self {
        Self {
            api,
            config,
            paths,
            window: PlaylistWindow::default(),
            mpv: None,
            mpv_tx,
            mpv_gen: 0,
            mpv_events: None,
            current: None,
            item_id: None,
            volume: 100,
            muted: false,
            paused: false,
            stopping: false,
            last_ticks: 0,
            external_subtitle_track_ids: HashMap::new(),
            audio: TrackState::default(),
            subtitle: TrackState::default(),
            report_tx,
            transitioning: false,
            mpv_auth_header_set: false,
            pending_start_ticks: None,
            prepared: HashMap::new(),
            titles: HashMap::new(),
        }
    }

    pub(super) async fn handle(&mut self, ev: CastEvent) -> color_eyre::Result<()> {
        match ev {
            CastEvent::PlayNow {
                item_ids,
                start_index,
                start_ticks,
                audio_stream_index,
                subtitle_stream_index,
                media_source_id,
            } => {
                tracing::info!(
                    n = item_ids.len(),
                    start_index,
                    audio_stream_index,
                    subtitle_stream_index,
                    ids = %item_ids.join(","),
                    "play now"
                );
                self.window.replace(item_ids, start_index);
                self.log_queue("play-now");
                self.start_current(&PlayRequest {
                    start_ticks,
                    audio_stream_index,
                    subtitle_stream_index,
                    media_source_id,
                })
                .await?;
            }
            CastEvent::PlayNext { item_ids } => self.enqueue(item_ids, Enqueue::Next).await?,
            CastEvent::PlayLast { item_ids } => self.enqueue(item_ids, Enqueue::Last).await?,
            CastEvent::PlayPause => self.toggle_pause().await?,
            CastEvent::Pause => self.apply_pause(true).await?,
            CastEvent::Unpause => self.apply_pause(false).await?,
            CastEvent::Stop => self.stop_playback(true).await,
            CastEvent::Seek { ticks } => self.seek_to(ticks).await?,
            CastEvent::Next => self.play_next_or_stop(false).await,
            CastEvent::Previous => self.play_previous().await?,
            CastEvent::SetVolume { volume } => self.apply_volume(volume).await?,
            CastEvent::VolumeUp => self.bump_volume(5).await?,
            CastEvent::VolumeDown => self.bump_volume(-5).await?,
            CastEvent::Mute => self.apply_mute(true).await?,
            CastEvent::Unmute => self.apply_mute(false).await?,
            CastEvent::ToggleMute => self.apply_mute(!self.muted).await?,
            CastEvent::SetAudio { stream_index } => {
                self.set_track(TrackKind::Audio, stream_index).await?
            }
            CastEvent::SetSubtitle { stream_index } => {
                self.set_track(TrackKind::Subtitle, stream_index).await?
            }
            CastEvent::ToggleFullscreen => {
                tracing::info!("toggle fullscreen");
                if let Some(mpv) = self.mpv.as_mut() {
                    mpv.toggle_fullscreen().await?;
                }
            }
        }
        Ok(())
    }

    /// PlayNext / PlayLast. With nothing playing these are just PlayNow;
    /// otherwise they extend the queue and top mpv's playlist up.
    async fn enqueue(&mut self, item_ids: Vec<String>, where_: Enqueue) -> color_eyre::Result<()> {
        tracing::info!(n = item_ids.len(), ?where_, ids = %item_ids.join(","), "enqueue");
        if self.mpv.is_none() {
            self.window.replace(item_ids, 0);
            return self.start_current(&PlayRequest::default()).await;
        }
        match where_ {
            Enqueue::Next => {
                self.window.insert_next(item_ids);
                self.log_queue("play-next-insert");
                // An mpv playlist holding later entries would need an
                // insert-at; the queue is right, mpv just does not show it yet.
                if self.window.tail() != 0 {
                    tracing::debug!(
                        tail = self.window.tail(),
                        "play-next not spliced into an already-appended mpv playlist"
                    );
                    return Ok(());
                }
            }
            Enqueue::Last => {
                self.window.append(item_ids);
                self.log_queue("play-last-append");
            }
        }
        self.fill_forward_into_mpv().await;
        Ok(())
    }

    async fn seek_to(&mut self, ticks: i64) -> color_eyre::Result<()> {
        tracing::info!(position_s = crate::ticks::ticks_to_seconds(ticks), "seek");
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.seek_absolute(crate::ticks::ticks_to_seconds(ticks))
                .await?;
            // The next progress tick may sample mid-seek; report the target now.
            self.last_ticks = ticks;
            self.send_progress();
        }
        Ok(())
    }

    /// Steps back within mpv's playlist when it has previous entries, and only
    /// otherwise restarts at the queue's previous item.
    async fn play_previous(&mut self) -> color_eyre::Result<()> {
        // Propagated rather than defaulted to 0, which would restart the
        // current item instead of stepping back.
        let playlist_pos = self.playlist_state().await?.map_or(0, |(pos, _)| pos);
        if playlist_pos == 0 {
            self.window.previous();
            return self.start_current(&PlayRequest::default()).await;
        }
        tracing::info!(playlist_pos, "playlist-prev");
        self.transitioning = true;
        let stepped = match self.mpv.as_mut() {
            Some(mpv) => mpv.playlist_prev().await,
            None => Ok(()),
        };
        if let Err(e) = stepped {
            // A stuck `transitioning` makes end_file_action ignore every
            // subsequent end-file.
            self.transitioning = false;
            tracing::error!("playlist-prev failed: {e:#}");
        }
        Ok(())
    }

    /// A track the remote picked: remember it, apply it, and report it back.
    async fn set_track(&mut self, kind: TrackKind, stream_index: i64) -> color_eyre::Result<()> {
        tracing::info!(kind = kind.as_str(), stream_index, "set stream");
        self.remember_track(kind, stream_index);
        self.apply_track(kind, stream_index).await?;
        self.send_progress();
        Ok(())
    }

    pub(super) async fn on_mpv_event(&mut self, ev: MpvEvent) {
        match ev {
            MpvEvent::FileLoaded => self.on_file_loaded().await,
            MpvEvent::SubtitleTrackChanged => self.adopt_mpv_track(TrackKind::Subtitle).await,
            MpvEvent::AudioTrackChanged => self.adopt_mpv_track(TrackKind::Audio).await,
            MpvEvent::EndFile { reason } => self.on_end_file(reason).await,
            MpvEvent::Exited => {
                if !self.stopping {
                    self.stop_playback(true).await;
                }
            }
        }
    }

    /// A new file is playing: adopt whatever mpv actually loaded (the user may
    /// have jumped in the selector), apply track choices, then resume-seek.
    async fn on_file_loaded(&mut self) {
        self.transitioning = false;
        if let Err(e) = self.adopt_playlist_pos().await {
            tracing::warn!("adopt playlist: {e:#}");
        }
        if let Err(e) = self.configure_streams().await {
            tracing::warn!("configure streams: {e:#}");
        }
        let Some(ticks) = self.pending_start_ticks.take() else {
            return;
        };
        let seconds = crate::ticks::ticks_to_seconds(ticks);
        tracing::info!(position_s = seconds, "resuming");
        if let Some(mpv) = self.mpv.as_mut()
            && let Err(e) = mpv.seek_absolute(seconds).await
        {
            tracing::warn!("resume seek: {e:#}");
        }
    }

    async fn on_end_file(&mut self, reason: EndFileReason) {
        tracing::info!(
            reason = %reason,
            transitioning = self.transitioning,
            stopping = self.stopping,
            has_next = self.window.has_next(),
            index = self.window.index(),
            queue = self.window.len(),
            origin = self.window.origin(),
            expected_pos = self.window.expected_pos(),
            "mpv end-file"
        );
        match end_file_action(self.transitioning, self.stopping, reason) {
            EndFileAction::Ignore => {}
            EndFileAction::Advance => self.play_next_or_stop(true).await,
            EndFileAction::Stop => self.stop_unless_playlist_moved(reason).await,
        }
    }

    /// `playlist-next` and an OSC jump both end the old file with `stop`, which
    /// is not a user Stop; a remaining playlist is what tells them apart.
    async fn stop_unless_playlist_moved(&mut self, reason: EndFileReason) {
        // A failed read means mpv has nothing left to hand us: fall through.
        let playlist_count = match self.playlist_state().await {
            Ok(state) => state.map_or(0, |(_, count)| count),
            Err(e) => {
                tracing::warn!("cannot read playlist-count: {e:#}");
                0
            }
        };
        if ignore_stop_for_playlist(reason, playlist_count) {
            tracing::debug!(
                playlist_count,
                "end-file stop while mpv still has a playlist; waiting for file-loaded"
            );
            return;
        }
        if reason == EndFileReason::Error {
            tracing::error!("mpv reported a playback error");
        }
        self.stop_playback(true).await;
    }
}
