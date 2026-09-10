use super::*;
use tempfile::TempDir;

#[test]
fn config_roundtrip_and_set() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
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
fn an_invalid_log_level_is_rejected_at_set_time() {
    let mut cfg = Config::default();
    // This used to be accepted and only fail at the next startup.
    let err = cfg.set(Field::LogLevel, "banana").unwrap_err();
    assert!(
        err.downcast_ref::<crate::UsageError>().is_some(),
        "expected UsageError, got {err:?}"
    );
    assert_eq!(cfg.log_level, "info", "the bad value must not be stored");
    cfg.set(Field::LogLevel, "jellysink=debug,warn").unwrap();
    assert_eq!(cfg.log_level, "jellysink=debug,warn");
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
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
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
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    paths.ensure().unwrap();
    fs::write(
        paths.config_file(),
        "mpv_path = \"mpv\"\nlog_level = \"info\"\n",
    )
    .unwrap();
    let loaded = Config::load(&paths).unwrap();
    assert!(loaded.autoplay);
}

#[test]
fn autoplay_defaults_on_and_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
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
