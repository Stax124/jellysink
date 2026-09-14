use super::*;
use tempfile::TempDir;

#[test]
fn config_roundtrip_and_set() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let mut cfg = Config::default();
    cfg.set(Field::MpvPath, "/usr/bin/mpv").unwrap();
    cfg.save(&paths).unwrap();
    let loaded = Config::load(&paths).unwrap();
    assert_eq!(loaded.mpv_path, "/usr/bin/mpv");
}

#[test]
fn unknown_config_key_errors_and_names_the_valid_ones() {
    let err = Field::parse("nope").unwrap_err();
    assert!(
        err.downcast_ref::<crate::UsageError>().is_some(),
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
    let mut cfg = Config::default();
    assert_eq!(cfg.get(Field::MpvArgs), None);
    assert!(
        !cfg.set(Field::MpvArgs, "--fullscreen").unwrap(),
        "the caller writes this to mpv_args.conf instead"
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
    let mut cfg = Config::default();
    let err = cfg.set(Field::Autoplay, "maybe").unwrap_err();
    assert!(
        err.downcast_ref::<crate::UsageError>().is_some(),
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
fn autoplay_defaults_on_and_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let cfg = Config::default();
    assert!(cfg.autoplay);
    cfg.save(&paths).unwrap();
    let loaded = Config::load(&paths).unwrap();
    assert!(loaded.autoplay);

    let mut cfg = loaded;
    cfg.set(Field::Autoplay, "false").unwrap();
    cfg.save(&paths).unwrap();
    let loaded = Config::load(&paths).unwrap();
    assert!(!loaded.autoplay);
    assert_eq!(loaded.get(Field::Autoplay).as_deref(), Some("false"));
}
