mod playback;
mod queue;
mod window;

use crate::cast::CastEvent;
use crate::config::{Config, Credentials, Paths};
use crate::jellyfin::auth::{Api, is_auth_expired};
use crate::jellyfin::session::{WsIncoming, parse_ws_message, websocket_url};
use crate::media::{AudioMemory, PlayRequest, PreparedPlay, SubtitleMemory};
use crate::mpv::EndFileReason;
use crate::mpv::{MpvEvent, MpvSession, SelectedTrack};
use crate::report::Report;
use crate::runtime::window::PlaylistWindow;
use crate::runtime::window::{EndFileAction, end_file_action, ignore_stop_for_playlist};
use crate::signal::Signal;
use color_eyre::eyre::{WrapErr, eyre};
use futures_util::{SinkExt, StreamExt};

/// Background tasks tied to the lifetime of whatever owns this.
///
/// Dropping it aborts them. Without it, a reconnect spawned a fresh WebSocket
/// reader and left the previous one running: against a half-open TCP
/// connection it never returns, so it leaked for the life of the process.
struct AbortOnDrop(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// Where PlayNext / PlayLast put their items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enqueue {
    /// Immediately after the current item.
    Next,
    /// At the end of the queue.
    Last,
}

type WsMessage = tokio_tungstenite::tungstenite::Message;
type WsError = tokio_tungstenite::tungstenite::Error;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// A session that stayed up at least this long is treated as having worked.
const SESSION_HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// How long to wait before the next reconnect attempt.
///
/// Without the healthy-session reset, `backoff` only ever grew: a few failures
/// at startup pinned it at [`BACKOFF_MAX`] for the rest of the process, so a
/// session that ran for hours and then dropped waited a full minute to come
/// back.
fn reconnect_delay(current: Duration, session_lasted: Duration, auth_expired: bool) -> Duration {
    if auth_expired {
        BACKOFF_MAX
    } else if session_lasted >= SESSION_HEALTHY_AFTER {
        BACKOFF_MIN
    } else {
        current
    }
}

/// The daemon loop: one long-lived player, a WebSocket that comes and goes.
///
/// `Runtime` is built once here and reused by every session, so a dropped
/// WebSocket is not visible to the user — mpv keeps playing, the queue, the
/// volume and the remembered tracks all stay put, and the next session picks
/// up exactly where the last one left off. Only the socket, its reader and the
/// keepalive are per-session; the mpv channel and the report sink are not, so
/// neither an mpv event nor a queued report is lost to a reconnect.
pub(crate) async fn run(
    config: Config,
    creds: Credentials,
    paths: Paths,
    shutdown: Signal,
) -> color_eyre::Result<()> {
    let mut backoff = BACKOFF_MIN;
    let api = Api::from_credentials(&creds)?;
    let (mpv_tx, mut mpv_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, MpvEvent)>();
    let (report_tx, report_task) = spawn_report_sink(Arc::new(api.clone()));
    // Aborted when `run` returns, however it returns.
    let _tasks = AbortOnDrop(vec![report_task]);
    let mut rt = Runtime::new(api, config, paths, mpv_tx, report_tx);
    loop {
        let started = Instant::now();
        match run_session(&mut rt, &mut mpv_rx, &shutdown).await {
            // Only shutdown ends a session cleanly.
            Ok(()) => break,
            Err(e) => {
                let auth_expired = is_auth_expired(&e);
                if auth_expired {
                    tracing::error!("{e:#}; staying idle until `jellysink login` is run again");
                } else {
                    tracing::warn!("session ended: {e:#}");
                }
                backoff = reconnect_delay(backoff, started.elapsed(), auth_expired);
            }
        }
        // Playback is deliberately left alone across the gap: nothing here
        // touches `rt`. Progress goes unreported until the next session (the
        // server is usually unreachable anyway), and mpv events raised while
        // we wait stay queued on `mpv_rx` for it to handle — the `mpv_gen` tag
        // is what keeps stale ones out.
        tokio::select! {
            _ = shutdown.fired() => break,
            _ = sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    // Not in a `select!` arm above: `run_session` holds `&mut rt`, so the
    // borrow checker will not let one of its arms touch `rt`.
    rt.stop_playback(true).await;
    Ok(())
}

/// Connects the remote-control WebSocket and pumps it into two channels.
///
/// The reader lives as long as the socket; a close or error ends it, which the
/// main loop sees as `ev_rx` closing.
fn spawn_ws_reader<S>(
    mut ws_read: S,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<CastEvent>,
    tokio::sync::mpsc::UnboundedReceiver<Duration>,
    tokio::task::JoinHandle<()>,
)
where
    S: StreamExt<Item = Result<WsMessage, WsError>> + Unpin + Send + 'static,
{
    let (ev_tx, ev_rx) = tokio::sync::mpsc::unbounded_channel::<CastEvent>();
    let (ka_tx, ka_rx) = tokio::sync::mpsc::unbounded_channel::<Duration>();
    let task = tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(WsMessage::Text(text)) => match parse_ws_message(&text) {
                    Ok(WsIncoming::Cast(ev)) => {
                        let _ = ev_tx.send(ev);
                    }
                    Ok(WsIncoming::ForceKeepAlive { seconds }) => {
                        let _ = ka_tx.send(Duration::from_secs((seconds / 2).max(1)));
                    }
                    Ok(WsIncoming::KeepAlive) => {}
                    Ok(WsIncoming::Ignored { message_type }) => {
                        tracing::debug!(message_type, "ignored websocket message");
                    }
                    Err(e) => tracing::debug!("ws parse: {e:#}"),
                },
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });
    (ev_rx, ka_rx, task)
}

/// Serialises session reports onto one task, so a Stopped can never overtake
/// the Start that preceded it.
fn spawn_report_sink(
    api: Arc<Api>,
) -> (
    tokio::sync::mpsc::UnboundedSender<Report>,
    tokio::task::JoinHandle<()>,
) {
    crate::report::spawn_reporter(move |report| {
        // Arc: this clone happens once per report, and Api holds six Strings.
        let api = Arc::clone(&api);
        async move {
            let r = match &report {
                Report::Start(s) => api.playing(s).await,
                Report::Progress(s) => api.progress(s).await,
                Report::Stopped(s) => api.stopped(s).await,
            };
            if let Err(e) = r {
                tracing::debug!("session report failed: {e:#}");
            }
        }
    })
}

/// One WebSocket session against the given, already-running [`Runtime`].
///
/// Returns `Ok(())` only for shutdown; any other end is an `Err` the caller
/// backs off and reconnects from. Errors deliberately leave `rt` — and so mpv,
/// the queue and the reported state — untouched.
async fn run_session(
    rt: &mut Runtime,
    mpv_rx: &mut tokio::sync::mpsc::UnboundedReceiver<(u64, MpvEvent)>,
    shutdown: &Signal,
) -> color_eyre::Result<()> {
    rt.api.post_capabilities().await?;

    let ws_url = websocket_url(&rt.api.server, &rt.api.token, &rt.api.device_id)?;
    tracing::info!("connecting websocket");
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .wrap_err("websocket connect")?;
    tracing::info!("websocket connected");
    let (mut ws_write, ws_read) = ws.split();

    let (mut ev_rx, mut ka_rx, ws_task) = spawn_ws_reader(ws_read);
    // Aborted when this returns, however it returns.
    let _tasks = AbortOnDrop(vec![ws_task]);

    rt.reannounce().await;

    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    let mut progress = tokio::time::interval(Duration::from_secs(1));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    progress.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.fired() => return Ok(()),
            _ = keepalive.tick() => {
                let msg = tokio_tungstenite::tungstenite::Message::Text(
                    json!({"MessageType":"KeepAlive"}).to_string().into(),
                );
                if ws_write.send(msg).await.is_err() {
                    return Err(eyre!("websocket send failed"));
                }
            }
            d = ka_rx.recv() => {
                match d {
                    Some(d) => {
                        keepalive = tokio::time::interval(d);
                        keepalive
                            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    }
                    // Both senders live in the WebSocket reader, so this closing
                    // means the reader is gone — the same condition `ev_rx`
                    // reports. Returning matters: a closed receiver is ready
                    // immediately and forever, so an empty body here left the
                    // arm permanently hot and spun the loop until `select!`
                    // happened to pick `ev_rx`.
                    None => return Err(eyre!("websocket closed")),
                }
            }
            _ = progress.tick() => {
                rt.tick_progress().await;
            }
            ev = ev_rx.recv() => {
                match ev {
                    Some(ev) => {
                        if let Err(e) = rt.handle(ev).await {
                            tracing::error!("cast command failed: {e:#}");
                        }
                    }
                    None => return Err(eyre!("websocket closed")),
                }
            }
            tagged = mpv_rx.recv() => {
                match tagged {
                    Some((generation, ev)) if generation == rt.mpv_gen => {
                        rt.on_mpv_event(ev).await;
                    }
                    // From a previous mpv session; see `mpv_gen`.
                    Some(_) => {}
                    // `rt` owns the matching sender for as long as `run`
                    // runs, so this cannot fire. Kept as a safety net rather
                    // than a panic, since the alternative in a daemon is worse.
                    None => return Err(eyre!("mpv event channel closed")),
                }
            }
        }
    }
}

struct Runtime {
    api: Api,
    config: Config,
    paths: Paths,
    /// The queue plus the window of it mpv currently holds.
    window: PlaylistWindow,
    mpv: Option<MpvSession>,
    mpv_tx: tokio::sync::mpsc::UnboundedSender<(u64, MpvEvent)>,
    /// Discriminates events from a previous mpv session that are still sitting
    /// on the shared channel. See `spawn_and_load`.
    mpv_gen: u64,
    /// The task forwarding the current mpv session's events. Owned so a respawn
    /// or a stop does not leave it running.
    mpv_events: Option<tokio::task::JoinHandle<()>>,
    current: Option<PreparedPlay>,
    item_id: Option<String>,
    volume: i64,
    muted: bool,
    paused: bool,
    stopping: bool,
    last_ticks: i64,
    /// Jellyfin subtitle stream index → mpv subtitle track id for `sub-add`ed files.
    external_subtitle_track_ids: HashMap<i64, i64>,
    /// The subtitle track the user last picked by hand, re-applied to the next
    /// episode by identity rather than by index. Deliberately never cleared by
    /// `start_current`, `adopt_playlist_pos` or `stop_playback`.
    last_subtitle: SubtitleMemory,
    /// mpv's subtitle selection as of the last time it was *ours* — the end of
    /// `configure_streams`, or an `apply_subtitle`. A `sid` property change
    /// reporting anything else is the user picking a track in the mpv window.
    settled_subtitle_track: SelectedTrack,
    /// The audio track the user last picked by hand, re-applied to the next
    /// episode by identity rather than by index. Never cleared, for the same
    /// reasons as `last_subtitle`.
    last_audio: AudioMemory,
    /// `settled_subtitle_track` for `aid`.
    settled_audio_track: SelectedTrack,
    report_tx: tokio::sync::mpsc::UnboundedSender<Report>,
    transitioning: bool,
    /// Whether mpv currently carries the Authorization header. When it does,
    /// playlist stubs leave the token off their URLs — mpv persists playlist
    /// entries to its watch_later files.
    mpv_auth_header_set: bool,
    pending_start_ticks: Option<i64>,

    prepared: HashMap<String, PreparedPlay>,
    /// Item id → display title from the series listing (and the current item).
    /// Playlist fill uses this; `PlaybackInfo` is fetched only when an item
    /// actually starts.
    titles: HashMap<String, String>,
}

impl Runtime {
    fn new(
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
            last_subtitle: SubtitleMemory::default(),
            settled_subtitle_track: SelectedTrack::Unresolved,
            last_audio: AudioMemory::default(),
            settled_audio_track: SelectedTrack::Unresolved,
            report_tx,
            transitioning: false,
            mpv_auth_header_set: false,
            pending_start_ticks: None,
            prepared: HashMap::new(),
            titles: HashMap::new(),
        }
    }

    async fn handle(&mut self, ev: CastEvent) -> color_eyre::Result<()> {
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
            CastEvent::SetAudio { stream_index } => self.set_audio(stream_index).await?,
            CastEvent::SetSubtitle { stream_index } => {
                tracing::info!(stream_index, "set subtitle stream");
                self.remember_subtitle(stream_index);
                self.apply_subtitle(stream_index).await?;
                self.send_progress();
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
                // Splicing into the middle of an mpv playlist that already
                // holds later entries would need an insert-at, not an append;
                // the queue is still correct, mpv just does not show it yet.
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

    /// Steps back within mpv's playlist when it has previous entries — which,
    /// with prepending on, is nearly always — and only otherwise restarts at
    /// the queue's previous item.
    async fn play_previous(&mut self) -> color_eyre::Result<()> {
        // Propagates on an IPC failure; `handle`'s caller logs it. Falling back
        // to 0 would restart the current item instead of stepping back.
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
            // See play_next_or_stop: a stuck `transitioning` makes
            // end_file_action Ignore every subsequent end-file.
            self.transitioning = false;
            tracing::error!("playlist-prev failed: {e:#}");
        }
        Ok(())
    }

    async fn set_audio(&mut self, stream_index: i64) -> color_eyre::Result<()> {
        tracing::info!(stream_index, "set audio stream");
        self.remember_audio(stream_index);
        self.apply_audio(stream_index).await?;
        self.send_progress();
        Ok(())
    }

    async fn on_mpv_event(&mut self, ev: MpvEvent) {
        match ev {
            MpvEvent::FileLoaded => self.on_file_loaded().await,
            MpvEvent::SubtitleTrackChanged => self.adopt_mpv_subtitle_track().await,
            MpvEvent::AudioTrackChanged => self.adopt_mpv_audio_track().await,
            MpvEvent::EndFile { reason } => self.on_end_file(reason).await,
            MpvEvent::Exited => {
                if !self.stopping {
                    self.stop_playback(true).await;
                }
            }
        }
    }

    /// A new file is playing: adopt whatever mpv actually loaded (the user may
    /// have jumped in the playlist selector), apply track choices, and seek to
    /// a pending resume offset.
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
    /// is not a user Stop. Distinguish them by whether mpv still has a playlist.
    async fn stop_unless_playlist_moved(&mut self, reason: EndFileReason) {
        // Already on the stop path: a failed read means mpv has nothing more to
        // hand us, so fall through to stopping.
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

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
