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

impl ksni::Tray for CastTray {
    fn id(&self) -> String {
        APP_NAME.into()
    }

    fn title(&self) -> String {
        APP_NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        jellyfin_icons()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Left-click is a no-op; quit via the menu, `jellysink stop`, or SIGTERM.
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
