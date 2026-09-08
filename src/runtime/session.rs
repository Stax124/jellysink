//! The daemon loop: connect, serve one WebSocket session, reconnect.

use super::state::Runtime;
use super::task::AbortOnDrop;
use crate::app::config::{Config, Credentials, Paths};
use crate::app::signal::Signal;
use crate::jellyfin::auth::{Api, is_auth_expired};
use crate::jellyfin::session::{WsIncoming, parse_ws_message, websocket_url};
use crate::mpv::MpvEvent;
use crate::report::Report;
use color_eyre::eyre::{WrapErr, eyre};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::time::sleep;

type WsMessage = tokio_tungstenite::tungstenite::Message;
type WsError = tokio_tungstenite::tungstenite::Error;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// A session that stayed up at least this long is treated as having worked.
const SESSION_HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// How long to wait before the next reconnect attempt. The healthy-session
/// reset is what stops a bad startup pinning the backoff at [`BACKOFF_MAX`]
/// for the rest of the process.
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
/// `Runtime` is built once and reused by every session, so a dropped WebSocket
/// is invisible to the user. Only the socket, its reader and the keepalive are
/// per-session — not the mpv channel or the report sink.
pub(crate) async fn run(
    config: Config,
    creds: Credentials,
    paths: Paths,
    shutdown: Signal,
    status_tx: tokio::sync::watch::Sender<super::status::PlayerStatus>,
) -> color_eyre::Result<()> {
    let mut backoff = BACKOFF_MIN;
    let api = Api::from_credentials(&creds)?;
    let (mpv_tx, mut mpv_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, MpvEvent)>();
    let (report_tx, report_task) = spawn_report_sink(api.clone());
    let _report_task = AbortOnDrop(report_task);
    let mut rt = Runtime::new(
        api,
        config,
        paths,
        mpv_tx,
        report_tx,
        creds.username,
        status_tx,
    );
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
        // Nothing here touches `rt`: playback rides out the gap, and mpv events
        // raised meanwhile stay queued on `mpv_rx` for the next session.
        tokio::select! {
            _ = shutdown.fired() => break,
            _ = sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    // Not in a `select!` arm above: `run_session` holds `&mut rt`.
    rt.stop_playback(true).await;
    Ok(())
}

/// The reader ends with the socket, which the main loop sees as the channel
/// closing.
fn spawn_ws_reader<S>(
    mut ws_read: S,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<WsIncoming>,
    tokio::task::JoinHandle<()>,
)
where
    S: StreamExt<Item = Result<WsMessage, WsError>> + Unpin + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<WsIncoming>();
    let task = tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(WsMessage::Text(text)) => match parse_ws_message(&text) {
                    // The receiver is gone: nothing left to read for.
                    Ok(incoming) => {
                        if tx.send(incoming).is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::debug!("ws parse: {e:#}"),
                },
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });
    (rx, task)
}

/// Serialises session reports onto one task, so a Stopped can never overtake
/// the Start that preceded it. The task owns the [`Api`], so reporting costs
/// neither a clone nor an `Arc` bump per report.
fn spawn_report_sink(
    api: Api,
) -> (
    tokio::sync::mpsc::UnboundedSender<Report>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Report>();
    let task = tokio::spawn(async move {
        while let Some(report) = rx.recv().await {
            let r = match &report {
                Report::Start(s) => api.playing(s).await,
                Report::Progress(s) => api.progress(s).await,
                Report::Stopped(s) => api.stopped(s).await,
            };
            if let Err(e) = r {
                tracing::debug!("session report failed: {e:#}");
            }
        }
    });
    (tx, task)
}

/// One WebSocket session against the given, already-running [`Runtime`].
/// `Ok(())` only for shutdown; every other end is an `Err` the caller
/// reconnects from, leaving `rt` untouched.
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

    let (mut ws_rx, ws_task) = spawn_ws_reader(ws_read);
    let _ws_task = AbortOnDrop(ws_task);

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
            _ = progress.tick() => {
                rt.tick_progress().await;
            }
            msg = ws_rx.recv() => {
                match msg {
                    Some(WsIncoming::Cast(ev)) => {
                        if let Err(e) = rt.handle(ev).await {
                            tracing::error!("cast command failed: {e:#}");
                        }
                    }
                    // The server dictates the interval; halve it so a tick is
                    // never the one that arrives late.
                    Some(WsIncoming::ForceKeepAlive { seconds }) => {
                        keepalive =
                            tokio::time::interval(Duration::from_secs((seconds / 2).max(1)));
                        keepalive
                            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    }
                    Some(WsIncoming::KeepAlive) => {}
                    Some(WsIncoming::Ignored { message_type }) => {
                        tracing::debug!(message_type, "ignored websocket message");
                    }
                    // The reader owns the sender, so this means it is gone.
                    // Must return: a closed receiver is ready forever, and an
                    // empty body here spins the loop.
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
                    // Unreachable while `run` runs; a daemon should not panic.
                    None => return Err(eyre!("mpv event channel closed")),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "session_test.rs"]
mod tests;
