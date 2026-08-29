use crate::APP_NAME;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};
use tokio::sync::Notify;

const JELLYFIN_ICO: &[u8] = include_bytes!("../assets/logo.ico");

pub struct Tray {
    pub handle: ksni::Handle<CastTray>,
    pub apply: Arc<Notify>,
}

pub struct CastTray {
    shutdown: Arc<Notify>,
    apply: Arc<Notify>,
    pending_version: Option<String>,
}

impl CastTray {
    pub fn set_pending(&mut self, version: String) {
        self.pending_version = Some(version);
    }
}

/// Load the icon from a potentially bundled ICO file
fn load_jellyfin_icons() -> Vec<ksni::Icon> {
    let dir = ico::IconDir::read(Cursor::new(JELLYFIN_ICO))
        .expect("bundled assets/logo.ico must be a valid ICO");
    dir.entries()
        .iter()
        .map(|entry| {
            let image = entry
                .decode()
                .expect("bundled assets/logo.ico entry must decode");
            let mut data = image.rgba_data().to_vec();
            for px in data.as_chunks_mut::<4>().0 {
                px.rotate_right(1); // RGBA -> ARGB32 network byte order
            }
            ksni::Icon {
                width: i32::try_from(image.width()).expect("icon width fits i32"),
                height: i32::try_from(image.height()).expect("icon height fits i32"),
                data,
            }
        })
        .collect()
}

fn jellyfin_icons() -> Vec<ksni::Icon> {
    static ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    ICONS.get_or_init(load_jellyfin_icons).clone()
}

fn jellyfin_update_icons() -> Vec<ksni::Icon> {
    static ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    ICONS
        .get_or_init(|| {
            jellyfin_icons()
                .into_iter()
                .map(with_update_badge)
                .collect()
        })
        .clone()
}

/// Corner badge so hosts that only display IconPixmap still show a pending update.
fn with_update_badge(mut icon: ksni::Icon) -> ksni::Icon {
    let w = icon.width;
    let h = icon.height;
    if w <= 0 || h <= 0 {
        return icon;
    }
    let r = (w.min(h) / 5).max(2);
    let cx = w - r - 1;
    let cy = h - r - 1;
    let outer2 = r.saturating_mul(r);
    let inner = (r * 3 / 4).max(1);
    let inner2 = inner.saturating_mul(inner);
    // ARGB32 network byte order
    const RING: [u8; 4] = [255, 255, 255, 255];
    const FILL: [u8; 4] = [255, 46, 204, 113];

    for y in 0..h {
        for x in 0..w {
            let dx = x - cx;
            let dy = y - cy;
            let d2 = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
            if d2 <= outer2 {
                let i = ((y as usize) * (w as usize) + (x as usize)) * 4;
                let px = if d2 <= inner2 { FILL } else { RING };
                icon.data[i..i + 4].copy_from_slice(&px);
            }
        }
    }
    icon
}

impl ksni::Tray for CastTray {
    // Left clicking the icon should open the menu as right clicking does, rather than doing nothing.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        APP_NAME.into()
    }

    fn title(&self) -> String {
        APP_NAME.into()
    }

    fn status(&self) -> ksni::Status {
        if self.pending_version.is_some() {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.pending_version.is_some() {
            jellyfin_update_icons()
        } else {
            jellyfin_icons()
        }
    }

    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.pending_version.is_some() {
            jellyfin_update_icons()
        } else {
            Vec::new()
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        match &self.pending_version {
            Some(v) => ksni::ToolTip {
                title: APP_NAME.into(),
                description: format!("Update available (v{v})"),
                ..Default::default()
            },
            None => Default::default(),
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let mut items = Vec::new();
        if let Some(v) = &self.pending_version {
            items.push(
                StandardItem {
                    label: format!("Install update (v{v})"),
                    activate: Box::new(|this: &mut Self| {
                        this.apply.notify_waiters();
                    }),
                    ..Default::default()
                }
                .into(),
            );
            items.push(MenuItem::Separator);
        }
        items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    this.shutdown.notify_waiters();
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Start the StatusNotifierItem. Fail-open: a missing tray host is a warning.
/// The returned handle must be kept alive for the tray to stay up and to
/// push a pending update into the menu.
pub async fn start(shutdown: Arc<Notify>) -> Option<Tray> {
    use ksni::TrayMethods;
    let apply = Arc::new(Notify::new());
    match (CastTray {
        shutdown,
        apply: apply.clone(),
        pending_version: None,
    })
    .spawn()
    .await
    {
        Ok(handle) => Some(Tray { handle, apply }),
        Err(e) => {
            tracing::warn!(
                "system tray unavailable ({e}); use `jellysink stop` or SIGTERM to quit"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    fn tray() -> CastTray {
        CastTray {
            shutdown: Arc::new(Notify::new()),
            apply: Arc::new(Notify::new()),
            pending_version: None,
        }
    }

    fn standard_labels(tray: &CastTray) -> Vec<String> {
        tray.menu()
            .into_iter()
            .filter_map(|item| match item {
                ksni::MenuItem::Standard(s) => Some(s.label),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn menu_is_quit_only_without_pending_update() {
        assert_eq!(standard_labels(&tray()), vec!["Quit".to_string()]);
    }

    #[test]
    fn menu_offers_install_when_pending() {
        let mut t = tray();
        t.set_pending("1.2.3".into());
        let items = t.menu();
        assert!(
            matches!(items.get(1), Some(ksni::MenuItem::Separator)),
            "install item should be followed by a separator"
        );
        assert_eq!(
            standard_labels(&t),
            vec!["Install update (v1.2.3)".to_string(), "Quit".to_string()]
        );
    }

    #[test]
    fn pending_update_requests_attention() {
        let mut t = tray();
        assert_eq!(t.status(), ksni::Status::Active);
        t.set_pending("1.2.3".into());
        assert_eq!(t.status(), ksni::Status::NeedsAttention);
    }

    #[test]
    fn pending_update_mentions_version_in_tooltip() {
        let mut t = tray();
        assert!(t.tool_tip().description.is_empty());
        t.set_pending("1.2.3".into());
        let tip = t.tool_tip();
        assert_eq!(tip.title, APP_NAME);
        assert!(
            tip.description.contains("1.2.3"),
            "tooltip should name the pending version, got {:?}",
            tip.description
        );
    }

    #[test]
    fn pending_update_badges_the_icon() {
        let idle = tray().icon_pixmap();
        let mut t = tray();
        t.set_pending("1.2.3".into());
        let pending = t.icon_pixmap();
        let attention = t.attention_icon_pixmap();

        let idle_large = idle
            .iter()
            .find(|i| i.width == 256 && i.height == 256)
            .unwrap();
        let pending_large = pending
            .iter()
            .find(|i| i.width == 256 && i.height == 256)
            .unwrap();
        assert_ne!(
            idle_large.data, pending_large.data,
            "pending icon should carry an update badge"
        );
        let center = ((128 * 256) + 128) * 4;
        assert_eq!(
            &idle_large.data[center..center + 4],
            &pending_large.data[center..center + 4],
            "badge should sit in a corner, not over the logo"
        );
        assert!(
            !attention.is_empty(),
            "NeedsAttention hosts should also get a badged attention icon"
        );
    }

    #[test]
    fn icon_pixmap_embeds_jellyfin_ico_as_argb32() {
        let icons = tray().icon_pixmap();
        assert!(
            !icons.is_empty(),
            "tray should ship the bundled Jellyfin icon"
        );

        let sizes: Vec<(i32, i32)> = icons.iter().map(|i| (i.width, i.height)).collect();
        assert!(
            sizes.contains(&(256, 256)),
            "logo.ico includes 256x256, got {sizes:?}"
        );

        for icon in &icons {
            assert!(icon.width > 0 && icon.height > 0);
            assert_eq!(
                icon.data.len(),
                (icon.width as usize) * (icon.height as usize) * 4
            );
        }

        let large = icons
            .iter()
            .find(|i| i.width == 256 && i.height == 256)
            .unwrap();
        let center = ((128 * 256) + 128) * 4;
        assert_eq!(&large.data[center..center + 4], &[255, 188, 119, 126]);
    }
}
