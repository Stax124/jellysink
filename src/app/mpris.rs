//! MPRIS (`org.mpris.MediaPlayer2`) media-key and desktop-widget integration.
//!
//! Two small interfaces sharing one [`Shared`] handle: reads mirror the same
//! [`PlayerStatus`] `jellysink status` uses, and every method just forwards a
//! [`CastEvent`] into `Runtime::handle` — the same dispatcher the WebSocket
//! path already uses. Fail-open like the tray: no session bus is a warning,
//! not a fatal error.

use crate::app::signal::Signal;
use crate::cast::CastEvent;
use crate::runtime::PlayerStatus;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.jellysink";

struct Shared {
    status_rx: watch::Receiver<PlayerStatus>,
    cmd_tx: mpsc::UnboundedSender<CastEvent>,
    shutdown: Signal,
}

impl Shared {
    fn send(&self, ev: CastEvent) {
        let _ = self.cmd_tx.send(ev);
    }
}

/// A stable, valid D-Bus object path derived from a Jellyfin item id (a GUID,
/// which contains hyphens — not legal in an object path segment).
fn track_id(item_id: &str) -> OwnedObjectPath {
    let sanitized: String = item_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    ObjectPath::try_from(format!("/org/jellysink/track/{sanitized}"))
        .expect("a sanitized item id is a valid object path segment")
        .into()
}

fn owned(value: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).expect("basic dbus values always convert")
}

struct RootIface(Arc<Shared>);

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl RootIface {
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "jellysink".to_string()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }

    fn raise(&self) {}

    fn quit(&self) {
        tracing::info!("mpris quit");
        self.0.shutdown.fire();
    }
}

struct PlayerIface(Arc<Shared>);

impl PlayerIface {
    fn status(&self) -> PlayerStatus {
        self.0.status_rx.borrow().clone()
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        match &self.status().now_playing {
            None => "Stopped",
            Some(np) if np.is_paused => "Paused",
            Some(_) => "Playing",
        }
        .to_string()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();
        if let Some(np) = &self.status().now_playing {
            map.insert("mpris:trackid".to_string(), owned(track_id(&np.item_id)));
            map.insert("xesam:title".to_string(), owned(np.title.clone()));
            map.insert("mpris:artUrl".to_string(), owned(np.art_url.clone()));
        }
        map
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.status()
            .now_playing
            .as_ref()
            .map_or(1.0, |np| np.volume as f64 / 100.0)
    }

    #[zbus(property)]
    fn set_volume(&self, value: f64) {
        self.0.send(CastEvent::SetVolume {
            volume: (value * 100.0).round() as i64,
        });
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.status()
            .now_playing
            .as_ref()
            .map_or(0, |np| np.position_ticks / 10)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.status().now_playing.is_some_and(|np| np.has_next)
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.status().now_playing.is_some_and(|np| np.has_previous)
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.status().now_playing.is_some()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.status().now_playing.is_some()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.status().now_playing.is_some()
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    fn play(&self) {
        self.0.send(CastEvent::Unpause);
    }

    fn pause(&self) {
        self.0.send(CastEvent::Pause);
    }

    fn play_pause(&self) {
        self.0.send(CastEvent::PlayPause);
    }

    fn stop(&self) {
        self.0.send(CastEvent::Stop);
    }

    fn next(&self) {
        self.0.send(CastEvent::Next);
    }

    fn previous(&self) {
        self.0.send(CastEvent::Previous);
    }

    /// `offset` is a relative microsecond delta, per the spec — not the
    /// absolute ticks [`CastEvent::Seek`] wants.
    fn seek(&self, offset: i64) {
        if let Some(np) = &self.status().now_playing {
            let ticks = np
                .position_ticks
                .saturating_add(offset.saturating_mul(10))
                .max(0);
            self.0.send(CastEvent::Seek { ticks });
        }
    }

    /// A no-op when `track_id` does not name the current track, per the spec.
    fn set_position(&self, track_id_arg: OwnedObjectPath, position: i64) {
        if let Some(np) = &self.status().now_playing
            && track_id(&np.item_id).as_str() == track_id_arg.as_str()
        {
            self.0.send(CastEvent::Seek {
                ticks: position.saturating_mul(10).max(0),
            });
        }
    }
}

async fn emit_changes(connection: zbus::Connection, mut status_rx: watch::Receiver<PlayerStatus>) {
    while status_rx.changed().await.is_ok() {
        let Ok(iface_ref) = connection
            .object_server()
            .interface::<_, PlayerIface>(OBJECT_PATH)
            .await
        else {
            return;
        };
        let iface = iface_ref.get().await;
        let ctx = iface_ref.signal_emitter();
        let _ = iface.playback_status_changed(ctx).await;
        let _ = iface.metadata_changed(ctx).await;
        let _ = iface.can_go_next_changed(ctx).await;
        let _ = iface.can_go_previous_changed(ctx).await;
        let _ = iface.can_play_changed(ctx).await;
        let _ = iface.can_pause_changed(ctx).await;
        let _ = iface.can_seek_changed(ctx).await;
        let _ = iface.volume_changed(ctx).await;
    }
}

/// Fail-open, exactly like the tray: no session bus (or another instance
/// already owning the MPRIS name) is a warning, not a fatal error.
pub(crate) async fn start(
    status_rx: watch::Receiver<PlayerStatus>,
    cmd_tx: mpsc::UnboundedSender<CastEvent>,
    shutdown: Signal,
) {
    let shared = Arc::new(Shared {
        status_rx: status_rx.clone(),
        cmd_tx,
        shutdown,
    });
    let build = async {
        zbus::connection::Builder::session()?
            .serve_at(OBJECT_PATH, RootIface(shared.clone()))?
            .serve_at(OBJECT_PATH, PlayerIface(shared))?
            .name(BUS_NAME)?
            .build()
            .await
    };
    match build.await {
        Ok(connection) => {
            tokio::spawn(emit_changes(connection, status_rx));
        }
        Err(e) => {
            tracing::warn!(
                "mpris unavailable ({e}); media keys and desktop widgets won't see jellysink"
            );
        }
    }
}

#[cfg(test)]
#[path = "mpris_test.rs"]
mod tests;
