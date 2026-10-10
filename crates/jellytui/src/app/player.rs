//! Driving the daemon: the one `control.sock` connection, the play shown as
//! starting until its outcome arrives, and what a pushed frame changes.

use super::*;
use jellysink_core::cast::CastEvent;
use jellysink_core::control::{Client, Command, DaemonFrame, Event, Origin, ReportKind, Request};

const RECONNECT_MIN: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(2);
const CLIENT: &str = concat!("jellytui ", env!("CARGO_PKG_VERSION"));

/// What the control socket last said. Until the first connect resolves, "no
/// daemon" is not yet a fact, so nothing may report one.
pub(crate) enum Daemon {
    Unknown,
    Absent,
    Connected(PlayerStatus),
}

/// A play sent from here that the daemon has not yet answered with an outcome.
pub(super) struct PendingPlay {
    request: u64,
    item_id: String,
    title: String,
}

/// The one connection to the daemon, made again with backoff for as long as
/// the loop runs.
pub(super) async fn stay_connected(
    paths: Paths,
    tx: UnboundedSender<Msg>,
    mut requests: UnboundedReceiver<Request>,
) {
    let mut backoff = RECONNECT_MIN;
    loop {
        // Sent before `App` saw the last connection die, and tracked by nobody
        // since: it must not run on this one.
        while requests.try_recv().is_ok() {}
        match Client::connect(&paths, CLIENT).await {
            Ok((client, status)) => {
                backoff = RECONNECT_MIN;
                let connected = Msg::Connected {
                    client_id: client.id(),
                    status: Box::new(status),
                };
                if tx.send(connected).is_err() || !forward(client, &tx, &mut requests).await {
                    return;
                }
            }
            Err(e) => tracing::debug!("connecting to jellysink: {e:#}"),
        }
        if tx.send(Msg::Disconnected).is_err() {
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// `false` once the loop is gone and there is nothing left to connect for.
async fn forward(
    mut client: Client,
    tx: &UnboundedSender<Msg>,
    requests: &mut UnboundedReceiver<Request>,
) -> bool {
    loop {
        tokio::select! {
            frame = client.next_frame() => match frame {
                Ok(Some(frame)) => {
                    if tx.send(Msg::Control(frame)).is_err() {
                        return false;
                    }
                }
                Ok(None) => return true,
                Err(e) => {
                    tracing::info!("lost jellysink: {e:#}");
                    return true;
                }
            },
            request = requests.recv() => match request {
                Some(request) => {
                    if let Err(e) = client.send(&request).await {
                        tracing::warn!("sending to jellysink: {e:#}");
                        return true;
                    }
                }
                None => return false,
            },
        }
    }
}

impl App {
    pub(super) fn on_connected(&mut self, client_id: u64, status: PlayerStatus) {
        // The transition, not every connect: startup's first one is not news.
        if matches!(self.daemon, Daemon::Absent) {
            tracing::info!(connected = true, "daemon");
        }
        self.client_id = Some(client_id);
        self.on_status(status);
    }

    pub(super) fn on_disconnected(&mut self) {
        if matches!(self.daemon, Daemon::Connected(_)) {
            tracing::info!(connected = false, "daemon");
        }
        self.daemon = Daemon::Absent;
        self.client_id = None;
        // The footer already says the daemon is gone.
        self.pending_play = None;
        self.follow_playing(None);
    }

    pub(super) fn on_frame(&mut self, frame: DaemonFrame) {
        tracing::trace!(?frame, "daemon frame");
        match frame {
            DaemonFrame::Status { status } => self.on_status(*status),
            DaemonFrame::Event(event) => self.on_daemon_event(event),
            DaemonFrame::Reply {
                id,
                error: Some(error),
            } => {
                tracing::warn!(id, %error, "jellysink refused a command");
                if self
                    .pending_play
                    .as_ref()
                    .is_some_and(|pending| pending.request == id)
                {
                    self.pending_play = None;
                    self.message = error;
                }
            }
            DaemonFrame::Error { message } => {
                tracing::warn!(%message, "jellysink could not read a command");
            }
            DaemonFrame::Welcome { .. } | DaemonFrame::Reply { error: None, .. } => {}
        }
    }

    fn on_status(&mut self, status: PlayerStatus) {
        let item_id = status
            .now_playing
            .as_ref()
            .map(|now_playing| now_playing.item_id.clone());
        self.daemon = Daemon::Connected(status);
        self.follow_playing(item_id);
    }

    fn follow_playing(&mut self, item_id: Option<String>) {
        let Some(item_id) = item_id else {
            self.playing_item = None;
            self.playing_requested = None;
            return;
        };
        if self.playing_requested.as_ref() != Some(&item_id) {
            self.load_playing(item_id);
        }
    }

    fn on_daemon_event(&mut self, event: Event) {
        match event {
            Event::PlayStarted { origin, .. } if self.answers_pending(origin) => {
                if let Some(pending) = self.pending_play.take() {
                    tracing::info!(item_id = %pending.item_id, "play started");
                }
            }
            Event::PlayCancelled { origin, by, .. } if self.answers_pending(origin) => {
                if let Some(pending) = self.pending_play.take() {
                    tracing::info!(item_id = %pending.item_id, ?by, "play overtaken");
                }
            }
            Event::PlayFailed { origin, error, .. } if self.answers_pending(origin) => {
                if let Some(pending) = self.pending_play.take() {
                    tracing::warn!(item_id = %pending.item_id, %error, "the daemon could not play it");
                    self.message = format!("jellysink could not play {}", pending.title);
                }
            }
            Event::Reported {
                report: ReportKind::Stopped,
                ..
            } => {
                tracing::info!(screen = ?self.screen, "reloading after a playback change");
                self.reload_screen_and_home();
            }
            _ => {}
        }
    }

    fn answers_pending(&self, origin: Origin) -> bool {
        match (&self.pending_play, self.client_id) {
            (Some(pending), Some(client)) => {
                origin
                    == Origin::Client {
                        client,
                        request: pending.request,
                    }
            }
            _ => false,
        }
    }

    /// The title of a play sent from here that the daemon is not playing yet.
    pub(crate) fn starting(&self) -> Option<&str> {
        let pending = self.pending_play.as_ref()?;
        (!self.is_current(&pending.item_id)).then_some(pending.title.as_str())
    }

    pub(crate) fn loading(&self) -> bool {
        matches!(&self.daemon, Daemon::Connected(status) if status.loading)
    }

    /// Home is reloaded whatever the screen: a watched item leaves Continue
    /// Watching and Next Up, and those are rarely the screen that is up.
    pub(super) fn reload_screen_and_home(&mut self) {
        self.reload_current_screen();
        if self.screen != Screen::Home {
            self.load_home();
        }
    }

    pub(super) fn is_current(&self, item_id: &str) -> bool {
        self.now_playing()
            .is_some_and(|now_playing| now_playing.item_id == item_id)
    }

    pub(super) fn play(&mut self, item: &Item) {
        // These share the header with the tabs, so they have to stay short
        // enough to survive `view::to_width` on an 80-column terminal.
        match self.daemon {
            Daemon::Unknown => {
                self.message = "checking for jellysink — try again".to_string();
                return;
            }
            Daemon::Absent => {
                self.message = "jellysink not connected".to_string();
                return;
            }
            Daemon::Connected(_) => {}
        }
        let (item_id, start_ticks) = (item.id.clone(), item.resume_ticks());
        tracing::info!(%item_id, title = item.name.as_deref().unwrap_or(""), start_ticks, "play");
        self.last_request += 1;
        let request = Request {
            id: self.last_request,
            command: Command::Cast(CastEvent::PlayNow {
                item_ids: vec![item_id.clone()],
                start_index: 0,
                start_ticks: Some(start_ticks),
                audio_stream_index: None,
                subtitle_stream_index: None,
                media_source_id: None,
            }),
        };
        if self.control_tx.send(request).is_err() {
            tracing::warn!("the jellysink connection task is gone");
            return;
        }
        self.pending_play = Some(PendingPlay {
            request: self.last_request,
            item_id,
            title: item.label(),
        });
    }

    pub(crate) fn now_playing(&self) -> Option<&jellysink_core::status::NowPlaying> {
        match &self.daemon {
            Daemon::Connected(status) => status.now_playing.as_ref(),
            Daemon::Unknown | Daemon::Absent => None,
        }
    }
}
