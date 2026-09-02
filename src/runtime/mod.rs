mod playback;
mod queue;

use crate::cast::{CastEvent, EndFileAction, Queue, end_file_action, ignore_stop_for_playlist};
use crate::config::{Config, Credentials, Paths};
use crate::jellyfin::auth::Api;
use crate::jellyfin::playback::PlaybackEndpoints;
use crate::jellyfin::session::{WsIncoming, parse_ws_message, websocket_url};
use crate::media::{self, PreparedPlay, mpv_aid};
use crate::mpv::{MpvEvent, MpvSession};
use crate::report::Report;
use color_eyre::eyre::{WrapErr, eyre};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::sleep;

pub async fn run(
    config: Config,
    creds: Credentials,
    paths: Paths,
    shutdown: Arc<Notify>,
) -> color_eyre::Result<()> {
    let mut backoff = Duration::from_secs(1);
    loop {
        tokio::select! {
            _ = shutdown.notified() => return Ok(()),
            result = run_session(&config, &creds, &paths, shutdown.clone()) => {
                match result {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.contains("401") {
                            tracing::error!(
                                "{msg}; staying idle until `jellysink login` is run again"
                            );
                            backoff = Duration::from_secs(60);
                        } else {
                            tracing::warn!("session ended: {msg}");
                        }
                    }
                }
            }
        }
        tokio::select! {
            _ = shutdown.notified() => return Ok(()),
            _ = sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

async fn run_session(
    config: &Config,
    creds: &Credentials,
    paths: &Paths,
    shutdown: Arc<Notify>,
) -> color_eyre::Result<()> {
    let api = Api::from_credentials(creds)?;
    let playback = PlaybackEndpoints::new(&api);
    playback.post_capabilities().await?;

    let ws_url = websocket_url(&api.server, &api.token, &api.device_id)?;
    tracing::info!("connecting websocket");
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .wrap_err("websocket connect")?;
    tracing::info!("websocket connected");
    let (mut ws_write, mut ws_read) = ws.split();

    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<CastEvent>();
    let (ka_tx, mut ka_rx) = tokio::sync::mpsc::unbounded_channel::<Duration>();
    let (mpv_tx, mut mpv_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, MpvEvent)>();

    tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                    match parse_ws_message(&text) {
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
                    }
                }
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let api_for_reports = api.clone();
    let report_tx = crate::report::spawn_reporter(move |report| {
        let api = api_for_reports.clone();
        async move {
            let endpoints = PlaybackEndpoints::new(&api);
            let r = match &report {
                Report::Start(s) => endpoints.playing(s).await,
                Report::Progress(s) => endpoints.progress(s).await,
                Report::Stopped(s) => endpoints.stopped(s).await,
            };
            if let Err(e) = r {
                tracing::debug!("session report failed: {e:#}");
            }
        }
    });

    let mut rt = Runtime {
        api,
        config: config.clone(),
        paths: paths.clone(),
        queue: Queue::default(),
        mpv: None,
        mpv_tx,
        mpv_gen: 0,
        current: None,
        item_id: None,
        volume: 100,
        muted: false,
        paused: false,
        stopping: false,
        last_ticks: 0,
        external_sid: HashMap::new(),
        report_tx,
        transitioning: false,
        pending_start_ticks: None,
        playlist_origin: 0,
        prepared: HashMap::new(),
        titles: HashMap::new(),
        mpv_tail: 0,
        mpv_head: 0,
        pending_prepend: Vec::new(),
    };

    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    let mut progress = tokio::time::interval(Duration::from_secs(1));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    progress.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                rt.stop_playback(true).await;
                return Ok(());
            }
            _ = keepalive.tick() => {
                let msg = tokio_tungstenite::tungstenite::Message::Text(
                    json!({"MessageType":"KeepAlive"}).to_string().into(),
                );
                if ws_write.send(msg).await.is_err() {
                    rt.stop_playback(true).await;
                    return Err(eyre!("websocket send failed"));
                }
            }
            d = ka_rx.recv() => {
                if let Some(d) = d {
                    keepalive = tokio::time::interval(d);
                    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
                    None => {
                        rt.stop_playback(true).await;
                        return Err(eyre!("websocket closed"));
                    }
                }
            }
            tagged = mpv_rx.recv() => {
                match tagged {
                    Some((generation, ev)) if generation == rt.mpv_gen => {
                        rt.on_mpv_event(ev).await;
                    }
                    Some(_) => {}
                    None => {
                        rt.stop_playback(true).await;
                        return Err(eyre!("mpv event channel closed"));
                    }
                }
            }
        }
    }
}

struct Runtime {
    api: Api,
    config: Config,
    paths: Paths,
    queue: Queue,
    mpv: Option<MpvSession>,
    mpv_tx: tokio::sync::mpsc::UnboundedSender<(u64, MpvEvent)>,
    mpv_gen: u64,
    current: Option<PreparedPlay>,
    item_id: Option<String>,
    volume: i64,
    muted: bool,
    paused: bool,
    stopping: bool,
    last_ticks: i64,
    external_sid: HashMap<i64, i64>,
    report_tx: tokio::sync::mpsc::UnboundedSender<Report>,
    transitioning: bool,
    pending_start_ticks: Option<i64>,
    playlist_origin: usize,
    prepared: HashMap<String, PreparedPlay>,
    /// Item id → display title from the series listing (and the current item).
    /// Playlist fill uses this; `PlaybackInfo` is fetched only when an item
    /// actually starts.
    titles: HashMap<String, String>,
    /// Queue entries already in mpv *after* the current one.
    mpv_tail: usize,
    /// Queue entries already in mpv *before* the current one (previous
    /// episodes spliced in). mpv's playlist is the contiguous window
    /// `items[origin .. origin + mpv_head + 1 + mpv_tail]`.
    mpv_head: usize,
    /// Previous episodes queued but not yet spliced into mpv. They are held
    /// until the current file is loaded, because `loadfile ... replace` wipes
    /// mpv's playlist.
    pending_prepend: Vec<String>,
}

impl Runtime {
    fn playback(&self) -> PlaybackEndpoints<'_> {
        PlaybackEndpoints::new(&self.api)
    }

    async fn handle(&mut self, ev: CastEvent) -> color_eyre::Result<()> {
        match ev {
            CastEvent::PlayNow {
                item_ids,
                start_index,
                start_ticks,
                aid,
                sid,
                srcid,
            } => {
                tracing::info!(
                    n = item_ids.len(),
                    start_index,
                    aid,
                    sid,
                    ids = %item_ids.join(","),
                    "play now"
                );
                self.queue.replace(item_ids, start_index);
                self.log_queue("play-now");
                self.start_current(start_ticks, aid, sid, srcid.as_deref())
                    .await?;
            }
            CastEvent::PlayNext { item_ids } => {
                tracing::info!(n = item_ids.len(), ids = %item_ids.join(","), "play next");
                if self.mpv.is_none() {
                    self.queue.replace(item_ids, 0);
                    self.start_current(None, None, None, None).await?;
                } else {
                    self.queue.insert_next(item_ids);
                    self.log_queue("play-next-insert");
                    if self.mpv_tail == 0 {
                        self.fill_forward_into_mpv().await;
                    } else {
                        tracing::debug!(
                            tail = self.mpv_tail,
                            "play-next not spliced into an already-appended mpv playlist"
                        );
                    }
                }
            }
            CastEvent::PlayLast { item_ids } => {
                tracing::info!(n = item_ids.len(), ids = %item_ids.join(","), "play last");
                if self.mpv.is_none() {
                    self.queue.replace(item_ids, 0);
                    self.start_current(None, None, None, None).await?;
                } else {
                    self.queue.append(item_ids);
                    self.log_queue("play-last-append");
                    self.fill_forward_into_mpv().await;
                }
            }
            CastEvent::PlayPause => self.toggle_pause().await?,
            CastEvent::Pause => self.apply_pause(true).await?,
            CastEvent::Unpause => self.apply_pause(false).await?,
            CastEvent::Stop => self.stop_playback(true).await,
            CastEvent::Seek { ticks } => {
                tracing::info!(position_s = media::ticks_to_seconds(ticks), "seek");
                if let Some(mpv) = self.mpv.as_mut() {
                    mpv.seek_absolute(media::ticks_to_seconds(ticks)).await?;
                    // The next progress tick may sample mid-seek; report the target now.
                    self.last_ticks = ticks;
                    self.send_progress();
                }
            }
            CastEvent::Next => self.play_next_or_stop(false).await,
            CastEvent::Previous => {
                let pos = if let Some(mpv) = self.mpv.as_mut() {
                    mpv.playlist_pos().await.unwrap_or(0)
                } else {
                    0
                };
                if pos > 0 {
                    tracing::info!(pos, "playlist-prev");
                    self.transitioning = true;
                    if let Some(mpv) = self.mpv.as_mut() {
                        let _ = mpv.playlist_prev().await;
                    }
                } else {
                    self.queue.previous();
                    self.start_current(None, None, None, None).await?;
                }
            }
            CastEvent::SetVolume { volume } => self.apply_volume(volume).await?,
            CastEvent::VolumeUp => self.bump_volume(5).await?,
            CastEvent::VolumeDown => self.bump_volume(-5).await?,
            CastEvent::Mute => self.apply_mute(true).await?,
            CastEvent::Unmute => self.apply_mute(false).await?,
            CastEvent::ToggleMute => self.apply_mute(!self.muted).await?,
            CastEvent::SetAudio { index } => {
                tracing::info!(index, "set audio stream");
                let aid = self.current.as_ref().and_then(|p| mpv_aid(&p.maps, index));
                if let (Some(mpv), Some(aid)) = (self.mpv.as_mut(), aid) {
                    mpv.set_aid(aid).await?;
                }
                if let Some(prep) = self.current.as_mut() {
                    prep.aid = Some(index);
                }
                self.send_progress();
            }
            CastEvent::SetSubtitle { index } => {
                tracing::info!(index, "set subtitle stream");
                self.apply_subtitle(index).await?;
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

    async fn on_mpv_event(&mut self, ev: MpvEvent) {
        match ev {
            MpvEvent::FileLoaded => {
                self.transitioning = false;
                if let Err(e) = self.adopt_playlist_pos().await {
                    tracing::warn!("adopt playlist: {e:#}");
                }
                if let Err(e) = self.configure_streams().await {
                    tracing::warn!("configure streams: {e:#}");
                }
                if let Some(ticks) = self.pending_start_ticks.take() {
                    tracing::info!(position_s = media::ticks_to_seconds(ticks), "resuming");
                    if let Some(mpv) = self.mpv.as_mut()
                        && let Err(e) = mpv.seek_absolute(media::ticks_to_seconds(ticks)).await
                    {
                        tracing::warn!("resume seek: {e:#}");
                    }
                }
            }
            MpvEvent::EndFile { reason } => {
                tracing::info!(
                    reason = %reason,
                    transitioning = self.transitioning,
                    stopping = self.stopping,
                    has_next = self.queue.has_next(),
                    index = self.queue.index,
                    queue = self.queue.items.len(),
                    origin = self.playlist_origin,
                    expected_pos = self.queue.index.saturating_sub(self.playlist_origin),
                    "mpv end-file"
                );
                match end_file_action(self.transitioning, self.stopping, &reason) {
                    EndFileAction::Ignore => {}
                    EndFileAction::Advance => self.play_next_or_stop(true).await,
                    EndFileAction::Stop => {
                        let count = if let Some(mpv) = self.mpv.as_mut() {
                            mpv.playlist_count().await.unwrap_or(0).max(0) as usize
                        } else {
                            0
                        };
                        if ignore_stop_for_playlist(&reason, count) {
                            tracing::debug!(
                                count,
                                "end-file stop while mpv still has a playlist; waiting for file-loaded"
                            );
                        } else {
                            if reason == "error" {
                                tracing::error!("mpv reported a playback error");
                            }
                            self.stop_playback(true).await;
                        }
                    }
                }
            }
            MpvEvent::Exited => {
                if !self.stopping {
                    self.stop_playback(true).await;
                }
            }
        }
    }
}
