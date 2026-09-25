use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn unknown_config_key_errors_and_names_the_valid_ones() {
    let err = Field::parse("nope").unwrap_err();
    assert!(
        err.downcast_ref::<crate::error::UsageError>().is_some(),
        "expected UsageError, got {err:?}"
    );
    let msg = err.to_string();
    for field in Field::ALL {
        assert!(msg.contains(field.name()), "{msg} should list {field:?}");
    }
}

#[test]
fn every_field_parses_back_from_its_own_name() {
    for field in Field::ALL {
        assert_eq!(Field::parse(field.name()).unwrap(), *field);
    }
}

#[test]
fn mpv_args_is_not_stored_in_config_toml() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    Field::MpvArgs
        .write(&paths, "--fullscreen --volume=50")
        .unwrap();
    assert_eq!(
        Field::MpvArgs.read(&paths).unwrap(),
        "--fullscreen --volume=50"
    );
    assert!(!paths.config_file().exists());
    assert_eq!(
        MpvArgs::load(&paths).unwrap().0,
        ["--fullscreen", "--volume=50"]
    );
}

#[test]
fn load_does_not_create_a_config_file() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let cfg = Config::load(&paths).unwrap();
    assert_eq!(cfg, Config::default());
    assert!(
        !paths.config_file().exists(),
        "`jellysink config path` should not write a config file"
    );
    Config::load_or_create(&paths).unwrap();
    assert!(paths.config_file().exists());
}

#[test]
fn invalid_autoplay_value_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let err = Field::Autoplay.write(&paths, "maybe").unwrap_err();
    assert!(
        err.downcast_ref::<crate::error::UsageError>().is_some(),
        "expected UsageError, got {err:?}"
    );
    assert!(err.to_string().contains("true/false"));
}

#[test]
fn missing_autoplay_key_defaults_on() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    paths.ensure().unwrap();
    fs::write(paths.config_file(), "mpv_path = \"mpv\"\n").unwrap();
    let loaded = Config::load(&paths).unwrap();
    assert!(loaded.autoplay);
}

/// Guards a config.toml written before `log_level` was removed.
#[test]
fn a_key_that_is_no_longer_a_field_is_ignored() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    paths.ensure().unwrap();
    fs::write(paths.config_file(), "log_level = \"debug\"\n").unwrap();
    assert_eq!(Config::load(&paths).unwrap(), Config::default());
}

#[test]
fn autoplay_off_survives_a_save_and_reload() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    Field::Autoplay.write(&paths, "false").unwrap();
    assert!(!Config::load(&paths).unwrap().autoplay);
    assert_eq!(Field::Autoplay.read(&paths).unwrap(), "false");
}
