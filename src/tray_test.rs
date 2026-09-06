use super::*;
use ksni::Tray;

fn tray() -> CastTray {
    CastTray {
        shutdown: Signal::new(),
        apply: Signal::new(),
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
fn pending_update_stays_active() {
    let mut t = tray();
    assert_eq!(t.status(), ksni::Status::Active);
    t.set_pending("1.2.3".into());
    assert_eq!(
        t.status(),
        ksni::Status::Active,
        "NeedsAttention makes hosts emphasize/resize the tray icon"
    );
    assert!(
        t.attention_icon_pixmap().is_empty(),
        "attention pixmap is what NeedsAttention hosts swap in"
    );
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

    assert_eq!(
        idle.iter().map(|i| (i.width, i.height)).collect::<Vec<_>>(),
        pending
            .iter()
            .map(|i| (i.width, i.height))
            .collect::<Vec<_>>(),
        "badge must not change pixmap dimensions"
    );

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
        t.attention_icon_pixmap().is_empty(),
        "do not ship an attention pixmap; hosts may swap size when emphasizing"
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
