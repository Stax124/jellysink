use super::*;
use tempfile::TempDir;

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
