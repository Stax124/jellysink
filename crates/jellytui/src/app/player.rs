//! Driving the daemon: the once-a-second status poll and the remote-control
//! commands a keypress turns into.

use super::*;

impl App {
    /// Footer state comes from the daemon's status socket, not `GET /Sessions`:
    /// that response embeds `NowPlayingQueueFullItems` and runs to megabytes
    /// once a series is queued, and no request parameter trims it.
    pub(super) fn poll_player(&self) {
        let (paths, tx) = (self.paths.clone(), self.tx.clone());
        tokio::spawn(async move {
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
            let _ = tx.send(Msg::Player(status.map(Box::new)));
        });
    }

    /// Once per process: see [`Msg::SessionId`].
    pub(super) fn load_session_id(&self) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match api.session_for_device().await {
                Ok(Some(session)) => {
                    tracing::info!(session_id = %session.id, "found the daemon's session");
                    Msg::SessionId(session.id)
                }
                Ok(None) => return,
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    pub(super) fn on_player(&mut self, player: Option<PlayerStatus>) {
        // The transition, not the poll: at 1 Hz the poll itself would fill the
        // buffer in half an hour.
        if self.player_polled && self.player.is_some() != player.is_some() {
            tracing::info!(connected = player.is_some(), "daemon");
        }
        let item_id = player
            .as_ref()
            .and_then(|status| status.now_playing.as_ref())
            .map(|now_playing| now_playing.item_id.clone());
        let changed = self.player_polled
            && item_id.as_deref()
                != self
                    .now_playing()
                    .map(|now_playing| now_playing.item_id.as_str());
        self.player_polled = true;
        self.player = player;
        // Firing before arming is what makes the deferral one poll rather than
        // none, so the read cannot outrun the daemon's report.
        if std::mem::take(&mut self.reload_due) {
            self.reload_after_playback();
        }
        self.reload_due = changed;
        let Some(item_id) = item_id else {
            self.runtime_ticks = None;
            self.playing_item = None;
            return;
        };
        if self
            .runtime_ticks
            .as_ref()
            .is_none_or(|(id, _)| *id != item_id)
        {
            self.load_runtime_ticks(item_id.clone());
            self.load_playing(item_id);
        }
    }

    /// Home is reloaded whatever the screen: finishing an episode invalidates
    /// Continue Watching and Next Up, rarely the screen that was up.
    fn reload_after_playback(&mut self) {
        tracing::info!(screen = ?self.screen, "reloading after a playback change");
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
        self.message = if self.player.is_some() {
            // Connected, but the one-off lookup has not landed yet.
            self.load_session_id();
            "looking up the session — try again".to_string()
        } else {
            "jellysink not connected".to_string()
        };
        None
    }

    pub(super) fn play(&mut self, item: &Item) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        let (item_id, start_ticks) = (item.id.clone(), item.resume_ticks());
        tracing::info!(%item_id, title = item.name.as_deref().unwrap_or(""), start_ticks, "play");
        tokio::spawn(async move {
            if let Err(e) = api.play_now(&session_id, &item_id, start_ticks).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    pub(crate) fn now_playing(&self) -> Option<&jellysink_core::status::NowPlaying> {
        self.player.as_ref()?.now_playing.as_ref()
    }

    /// The current item's duration, only if it belongs to the current item.
    pub(crate) fn total_ticks(&self) -> Option<i64> {
        let now_playing = self.now_playing()?;
        self.runtime_ticks
            .as_ref()
            .filter(|(item_id, _)| *item_id == now_playing.item_id)
            .map(|(_, ticks)| *ticks)
    }
}
