//! Driving the daemon: the once-a-second status poll and the remote-control
//! commands a keypress turns into.

use super::*;

impl App {
    /// Footer state comes from the daemon's own status socket, not from
    /// `GET /Sessions`: that response embeds `NowPlayingQueueFullItems` and
    /// runs to megabytes once a series is queued, which cannot be trimmed by
    /// any request parameter. This is a few dozen bytes over a Unix socket.
    pub(super) fn poll_player(&self) {
        let (paths, tx) = (self.paths.clone(), self.tx.clone());
        tokio::spawn(async move {
            let status = tokio::task::spawn_blocking(move || instance::request_status(&paths))
                .await
                .ok()
                .and_then(Result::ok);
            let _ = tx.send(Msg::Player(status.map(Box::new)));
        });
    }

    /// Once per process: see [`Msg::SessionId`].
    pub(super) fn load_session_id(&self) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match api.session_for_device().await {
                Ok(Some(session)) => Msg::SessionId(session.id),
                Ok(None) => return,
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    pub(super) fn on_player(&mut self, player: Option<PlayerStatus>) {
        self.player_polled = true;
        let item_id = player
            .as_ref()
            .and_then(|status| status.now_playing.as_ref())
            .map(|now_playing| now_playing.item_id.clone());
        self.player = player;
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
        tokio::spawn(async move {
            if let Err(e) = api.play_now(&session_id, &item_id, start_ticks).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    pub(super) fn send_playstate(&mut self, command: PlaystateCommand, seek_ticks: Option<i64>) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            if let Err(e) = api.playstate(&session_id, command, seek_ticks).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    pub(super) fn send_general(&mut self, name: &'static str, arguments: serde_json::Value) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            if let Err(e) = api.general_command(&session_id, name, arguments).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    pub(super) fn seek_by(&mut self, seconds: i64) {
        let Some(position) = self.position_ticks() else {
            return;
        };
        self.send_playstate(PlaystateCommand::Seek, Some(seek_target(position, seconds)));
    }

    pub(super) fn volume_by(&mut self, delta: i64) {
        let current = self
            .player
            .as_ref()
            .and_then(|status| status.now_playing.as_ref())
            .map_or(100, |now_playing| now_playing.volume);
        let volume = (current + delta).clamp(0, 100);
        self.send_general("SetVolume", json!({ "Volume": volume.to_string() }));
    }

    pub(crate) fn now_playing(&self) -> Option<&jellysink_core::status::NowPlaying> {
        self.player.as_ref()?.now_playing.as_ref()
    }

    pub(crate) fn position_ticks(&self) -> Option<i64> {
        self.now_playing()
            .map(|now_playing| now_playing.position_ticks)
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

/// Seeking is absolute over the wire, so the target is computed from the last
/// polled position — up to a second stale, which at ten-second steps is not
/// noticeable.
pub(super) fn seek_target(position_ticks: i64, seconds: i64) -> i64 {
    (position_ticks + seconds_to_ticks(seconds as f64)).max(0)
}
