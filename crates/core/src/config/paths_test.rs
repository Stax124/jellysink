use super::*;

#[test]
fn ensure_makes_the_config_dir_private() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_override(Some(dir.path().join("jellysink"))).unwrap();
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
    let paths = Paths::from_override(Some(dir.path().join("jellysink"))).unwrap();
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

#[test]
fn an_overridden_config_dir_takes_the_cover_cache_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    assert!(paths.cover_cache_dir().starts_with(dir.path()));
}
