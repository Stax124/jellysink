use super::*;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

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
