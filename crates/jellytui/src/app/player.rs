//! Driving the daemon: the once-a-second status poll and the remote-control
//! commands a keypress turns into.

use super::*;

/// What the status socket last said. Until the first poll answers, "no
/// daemon" is not yet a fact, so nothing may report one.
pub(crate) enum Daemon {
    Unknown,
    Absent,
    Connected(PlayerStatus),
}

impl App {
    /// The status socket rather than `GET /Sessions`; see `specs/tui.md`.
    pub(super) fn poll_player(&self) {
        let paths = self.paths.clone();
        self.spawn_msg(async move {
            let started = std::time::Instant::now();
            let status = tokio::task::spawn_blocking(move || instance::request_status(&paths))
                .await
                .ok()
                .and_then(Result::ok);
            tracing::trace!(
                answered = status.is_some(),
                elapsed_ms = started.elapsed().as_millis(),
                "status poll"
            );
            Some(Msg::Player(status.map(Box::new)))
        });
    }

    /// Once per process: see [`Msg::SessionId`].
    pub(super) fn load_session_id(&self) {
        let api = self.api.clone();
        self.spawn_msg(async move {
            match api.session_for_device().await {
                Ok(Some(session)) => {
                    tracing::info!(session_id = %session.id, "found the daemon's session");
                    Some(Msg::SessionId(session.id))
                }
                Ok(None) => None,
                Err(e) => Some(e.into()),
            }
        });
    }

    pub(super) fn on_player(&mut self, player: Option<PlayerStatus>) {
        let first_poll = matches!(self.daemon, Daemon::Unknown);
        // The transition, not the poll: at 1 Hz the poll itself would fill the
        // buffer in half an hour.
        if !first_poll && matches!(self.daemon, Daemon::Connected(_)) != player.is_some() {
            tracing::info!(connected = player.is_some(), "daemon");
        }
        let item_id = player
            .as_ref()
            .and_then(|status| status.now_playing.as_ref())
            .map(|now_playing| now_playing.item_id.clone());
        let changed = !first_poll
            && item_id.as_deref()
                != self
                    .now_playing()
                    .map(|now_playing| now_playing.item_id.as_str());
        self.daemon = match player {
            Some(status) => Daemon::Connected(status),
            None => Daemon::Absent,
        };
        // Firing before arming is what makes the deferral one poll rather than
        // none, so the read cannot outrun the daemon's report.
        if std::mem::take(&mut self.reload_due) {
            tracing::info!(screen = ?self.screen, "reloading after a playback change");
            self.reload_screen_and_home();
        }
        self.reload_due = changed;
        let Some(item_id) = item_id else {
            self.playing_item = None;
            self.playing_requested = None;
            return;
        };
        if self.playing_requested.as_ref() != Some(&item_id) {
            self.load_playing(item_id);
        }
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

    pub(super) fn session_id(&mut self) -> Option<String> {
        if let Some(session_id) = &self.session_id {
            return Some(session_id.clone());
        }
        // These share the header with the tabs, so they have to stay short
        // enough to survive `view::to_width` on an 80-column terminal.
        self.message = match self.daemon {
            Daemon::Unknown => "checking for jellysink — try again",
            Daemon::Absent => "jellysink not connected",
            Daemon::Connected(_) => {
                // Connected, but the one-off lookup has not landed yet.
                self.load_session_id();
                "looking up the session — try again"
            }
        }
        .to_string();
        None
    }

    pub(super) fn play(&mut self, item: &Item) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let api = self.api.clone();
        let (item_id, start_ticks) = (item.id.clone(), item.resume_ticks());
        tracing::info!(%item_id, title = item.name.as_deref().unwrap_or(""), start_ticks, "play");
        self.spawn_msg(async move {
            api.play_now(&session_id, &item_id, start_ticks)
                .await
                .err()
                .map(Msg::from)
        });
    }

    pub(crate) fn now_playing(&self) -> Option<&jellysink_core::status::NowPlaying> {
        match &self.daemon {
            Daemon::Connected(status) => status.now_playing.as_ref(),
            Daemon::Unknown | Daemon::Absent => None,
        }
    }
}
