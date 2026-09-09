use super::*;

#[test]
fn after_install_restarts_from_tray_when_running() {
    assert_eq!(after_install(true, true, true), AfterInstall::Restart);
}

#[test]
fn after_install_stops_from_cli_when_running() {
    assert_eq!(after_install(false, true, true), AfterInstall::Stop);
}

#[test]
fn after_install_does_nothing_when_not_running_or_not_updated() {
    assert_eq!(after_install(true, false, true), AfterInstall::None);
    assert_eq!(after_install(false, true, false), AfterInstall::None);
    assert_eq!(after_install(true, true, false), AfterInstall::None);
}

/// `config set` is the only writer of config.toml, so its help text is where a
/// user finds the key names; a new `Field` that is not listed there is invisible.
#[test]
fn the_cli_help_lists_every_config_key() {
    let main_rs = include_str!("../main.rs");
    for field in Field::ALL {
        assert!(
            main_rs.contains(field.name()),
            "src/main.rs should mention {:?} in the `config set` help",
            field.name()
        );
    }
}
