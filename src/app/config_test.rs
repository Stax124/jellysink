use super::*;
#[test]
fn ensure_makes_the_config_dir_private() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config_dir: dir.path().join("jellysink"),
    };
    paths.ensure().unwrap();
    let mode = fs::metadata(&paths.config_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "mpv.sock lives here and leaks the token");
}

#[test]
fn ensure_tightens_a_directory_left_world_readable_by_an_older_version() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config_dir: dir.path().join("jellysink"),
    };
    fs::create_dir_all(&paths.config_dir).unwrap();
    fs::set_permissions(&paths.config_dir, fs::Permissions::from_mode(0o755)).unwrap();
    paths.ensure().unwrap();
    let mode = fs::metadata(&paths.config_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

use tempfile::TempDir;

#[test]
fn bare_host_gets_http_and_8096() {
    assert_eq!(
        normalize_server_url("192.168.1.10").unwrap(),
        "http://192.168.1.10:8096"
    );
}

#[test]
fn explicit_port_80_is_kept() {
    assert_eq!(
        normalize_server_url("http://media.local:80").unwrap(),
        "http://media.local:80"
    );
}

#[test]
fn https_without_port_is_not_given_8096() {
    assert_eq!(
        normalize_server_url("https://jellyfin.example").unwrap(),
        "https://jellyfin.example"
    );
}

#[test]
fn subpath_is_kept() {
    assert_eq!(
        normalize_server_url("http://host:8096/jellyfin/").unwrap(),
        "http://host:8096/jellyfin"
    );
}

#[test]
fn scheme_typo_without_slashes_is_rejected() {
    assert!(normalize_server_url("http//").is_err());
    assert!(normalize_server_url("http/media.local").is_err());
    assert!(normalize_server_url("https:").is_err());
    assert!(normalize_server_url("http").is_err());
}

#[test]
fn cred_file_is_mode_600() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    let creds = Credentials {
        server: "http://h:8096".into(),
        username: "u".into(),
        user_id: "id".into(),
        access_token: "tok".into(),
        device_id: "dev".into(),
    };
    creds.save(&paths).unwrap();
    let mode = fs::metadata(paths.cred_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(Credentials::load(&paths).unwrap().unwrap(), creds);
}

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
fn mpv_args_roundtrip_and_reload() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    MpvArgs::save(&paths, "--hwdec=no --vo=gpu").unwrap();
    let loaded = MpvArgs::load(&paths).unwrap();
    assert_eq!(loaded.0, vec!["--hwdec=no", "--vo=gpu"]);
    assert_eq!(MpvArgs::get(&paths).unwrap(), "--hwdec=no --vo=gpu");

    // A running daemon re-reads the file; edits must be visible.
    MpvArgs::save(&paths, "--fullscreen").unwrap();
    assert_eq!(MpvArgs::load(&paths).unwrap().0, vec!["--fullscreen"]);
}

#[test]
fn mpv_args_missing_file_is_empty() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    assert_eq!(MpvArgs::load(&paths).unwrap().0, Vec::<String>::new());
}

#[test]
fn mpv_args_comments_and_blank_lines_ignored() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    fs::write(
        paths.mpv_args_file(),
        "# comment\n\n--fullscreen\n  \n--volume=50\n",
    )
    .unwrap();
    assert_eq!(
        MpvArgs::load(&paths).unwrap().0,
        vec!["--fullscreen", "--volume=50"]
    );
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

/// `--help` spells the key list out; keep it honest.
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

#[test]
fn credentials_debug_never_prints_the_access_token() {
    let creds = Credentials {
        server: "http://s".into(),
        username: "u".into(),
        user_id: "uid".into(),
        access_token: "sekrit".into(),
        device_id: "d".into(),
    };
    let rendered = format!("{creds:?}");
    assert!(!rendered.contains("sekrit"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}
