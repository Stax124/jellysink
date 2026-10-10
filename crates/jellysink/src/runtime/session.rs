//! The daemon loop: connect, serve one WebSocket session, reconnect.

use super::state::Runtime;
use super::task::AbortOnDrop;
use crate::daemon::signal::Signal;
use crate::jellyfin;
use crate::mpv::{MpvEvent, MpvSession};
use crate::report::Report;
use color_eyre::eyre::{WrapErr, eyre};
use futures_util::{SinkExt, StreamExt};
use jellysink_core::cast::CastEvent;
use jellysink_core::config::{Config, Credentials, Paths};
use jellysink_core::control::{Event, Origin, ReportKind};
use jellysink_core::jellyfin::auth::{Api, is_auth_expired};
use jellysink_core::jellyfin::session::{WsIncoming, parse_ws_message, websocket_url};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::time::{Interval, MissedTickBehavior, sleep};

type WsMessage = tokio_tungstenite::tungstenite::Message;
type WsError = tokio_tungstenite::tungstenite::Error;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// A cast from MPRIS or a control client, and who sent it.
#[derive(Debug, PartialEq)]
pub(crate) struct CastCommand {
    pub(crate) event: CastEvent,
    pub(crate) origin: Origin,
}

/// A session that stayed up at least this long is treated as having worked.
const SESSION_HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// The healthy-session reset stops a bad startup pinning the backoff at
/// [`BACKOFF_MAX`].
fn reconnect_delay(current: Duration, session_lasted: Duration, auth_expired: bool) -> Duration {
    if auth_expired {
        BACKOFF_MAX
    } else if session_lasted >= SESSION_HEALTHY_AFTER {
        BACKOFF_MIN
    } else {
        current
    }
}

/// Only the socket, its reader and the keepalive are per-session, so a dropped
/// WebSocket is invisible to the user. See `specs/session.md`.
pub(crate) async fn run(
    config: Config,
    creds: Credentials,
    paths: Paths,
    shutdown: Signal,
    status_tx: tokio::sync::watch::Sender<jellysink_core::status::PlayerStatus>,
    event_tx: broadcast::Sender<Event>,
    mut command_rx: tokio::sync::mpsc::UnboundedReceiver<CastCommand>,
) -> color_eyre::Result<()> {
    let mut backoff = BACKOFF_MIN;
    let api = Api::from_credentials(&creds)?;
    let (report_tx, _report_task) = spawn_report_sink(api.clone(), event_tx.clone());
    let mut rt = Runtime::new(
        api,
        config,
        paths,
        report_tx,
        creds.username,
        status_tx,
        event_tx,
    );
    loop {
        let started = Instant::now();
        let ended = run_session(&mut rt, &mut command_rx, &shutdown).await;
        rt.server_connected = false;
        rt.publish_status();
        match ended {
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
        // raised meanwhile stay queued on its `MpvSession` for the next session.
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
    AbortOnDrop,
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
                Ok(WsMessage::Close(_)) => break,
                Err(e) => {
                    tracing::warn!("websocket read failed: {e}");
                    break;
                }
                _ => {}
            }
        }
    });
    (rx, AbortOnDrop(task))
}

/// Serialises session reports onto one task, so a Stopped can never overtake
/// the Start before it. The task owns the [`Api`], so a report costs no clone.
fn spawn_report_sink(
    api: Api,
    event_tx: broadcast::Sender<Event>,
) -> (tokio::sync::mpsc::UnboundedSender<Report>, AbortOnDrop) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Report>();
    let task = tokio::spawn(async move {
        while let Some(report) = rx.recv().await {
            let posted = jellyfin::post_report(&api, &report).await;
            if let Err(e) = &posted {
                tracing::warn!("session report failed: {e:#}");
            }
            let kind = match &report {
                Report::Start(_) => ReportKind::Start,
                Report::Stopped(_) => ReportKind::Stopped,
                Report::Progress(_) => continue,
            };
            // Fails only while no client is listening.
            let _ = event_tx.send(Event::Reported {
                report: kind,
                item_id: report.state().item_id.clone(),
                ok: posted.is_ok(),
            });
        }
    });
    (tx, AbortOnDrop(task))
}

/// Delays rather than skips a late tick: a late keepalive still has to be sent.
fn keepalive_interval(period: Duration) -> Interval {
    let mut keepalive = tokio::time::interval(period);
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    keepalive
}

/// Pends forever with no mpv, so the arm is idle between plays.
async fn next_mpv_event(mpv: &mut Option<MpvSession>) -> MpvEvent {
    match mpv {
        Some(mpv) => mpv.next_event().await,
        None => std::future::pending().await,
    }
}

/// One WebSocket session against an already-running [`Runtime`]. `Ok(())` only
/// for shutdown; every other end is an `Err` the caller reconnects from.
async fn run_session(
    rt: &mut Runtime,
    command_rx: &mut tokio::sync::mpsc::UnboundedReceiver<CastCommand>,
    shutdown: &Signal,
) -> color_eyre::Result<()> {
    crate::jellyfin::post_capabilities(&rt.api).await?;

    let ws_url = websocket_url(&rt.api.server, &rt.api.token, &rt.api.device_id)?;
    tracing::info!("connecting websocket");
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .wrap_err_with(|| format!("connecting the websocket to {}", rt.api.server))?;
    tracing::info!("websocket connected");
    let (mut ws_write, ws_read) = ws.split();

    let (mut ws_rx, _ws_task) = spawn_ws_reader(ws_read);

    rt.server_connected = true;
    rt.publish_status();
    rt.reannounce().await;

    let mut keepalive = keepalive_interval(Duration::from_secs(30));
    let mut progress = tokio::time::interval(Duration::from_secs(1));
    progress.set_missed_tick_behavior(MissedTickBehavior::Skip);

    // Not fatal like the websocket channel above: MPRIS and the control socket
    // are both optional.
    let mut commands_closed = false;

    loop {
        tokio::select! {
            _ = shutdown.fired() => return Ok(()),
            _ = keepalive.tick() => {
                let msg = WsMessage::Text(json!({"MessageType":"KeepAlive"}).to_string().into());
                ws_write
                    .send(msg)
                    .await
                    .wrap_err_with(|| format!("sending a keepalive to {}", rt.api.server))?;
            }
            _ = progress.tick() => {
                rt.tick_progress().await;
            }
            msg = ws_rx.recv() => {
                match msg {
                    Some(WsIncoming::Cast(ev)) => {
                        if let Err(e) = rt.handle(ev, Origin::Jellyfin).await {
                            tracing::error!("cast command failed: {e:#}");
                        }
                    }
                    // The server dictates the interval; halve it so a tick is
                    // never the one that arrives late.
                    Some(WsIncoming::ForceKeepAlive { seconds }) => {
                        keepalive = keepalive_interval(Duration::from_secs((seconds / 2).max(1)));
                    }
                    Some(WsIncoming::KeepAlive) => {}
                    Some(WsIncoming::Ignored { message_type }) => {
                        tracing::debug!(message_type, "ignored websocket message");
                    }
                    // The reader owns the sender, so this means it is gone. Must
                    // return: a closed receiver is ready forever and spins the loop.
                    None => return Err(eyre!("websocket closed")),
                }
            }
            ev = next_mpv_event(&mut rt.mpv) => {
                rt.on_mpv_event(ev).await;
            }
            command = command_rx.recv(), if !commands_closed => {
                match command {
                    Some(CastCommand { event, origin }) => {
                        if let Err(e) = rt.handle(event, origin).await {
                            tracing::error!(?origin, "command failed: {e:#}");
                        }
                    }
                    None => commands_closed = true,
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "session_test.rs"]
mod tests;
