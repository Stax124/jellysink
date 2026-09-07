use crate::APP_NAME;
use crate::signal::Signal;
use std::io::Cursor;
use std::sync::OnceLock;

const JELLYFIN_ICO: &[u8] = include_bytes!("../assets/logo.ico");

pub(crate) struct Tray {
    pub(crate) handle: ksni::Handle<CastTray>,
    pub(crate) apply: Signal,
}

pub(crate) struct CastTray {
    shutdown: Signal,
    apply: Signal,
    pending_version: Option<String>,
}

impl CastTray {
    pub(crate) fn set_pending(&mut self, version: String) {
        self.pending_version = Some(version);
    }
}

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

/// The clone is the trait's; the `OnceLock` keeps the ICO decode to once.
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
    // Left click should open the menu, as right click does.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        APP_NAME.into()
    }

    fn title(&self) -> String {
        APP_NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.pending_version.is_some() {
            jellyfin_update_icons()
        } else {
            jellyfin_icons()
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
                        this.apply.fire();
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
                    this.shutdown.fire();
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Start the StatusNotifierItem. Fail-open: a missing tray host is a warning.
/// The handle must be kept alive for the tray to stay up.
pub(crate) async fn start(shutdown: Signal) -> Option<Tray> {
    use ksni::TrayMethods;
    let apply = Signal::new();
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
#[path = "tray_test.rs"]
mod tests;
