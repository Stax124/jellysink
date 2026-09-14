use super::*;

#[test]
fn release_target_is_linux_musl_for_this_arch() {
    let t = release_target().expect("jellysink only ships linux musl for x86_64/aarch64");
    assert!(t.ends_with("-unknown-linux-musl"), "{t}");
    assert!(t.starts_with(std::env::consts::ARCH), "{t}");
}

#[test]
fn restart_exe_path_strips_linux_deleted_suffix() {
    let p = Path::new("/home/u/.local/bin/jellysink (deleted)");
    assert_eq!(
        restart_exe_path(p),
        PathBuf::from("/home/u/.local/bin/jellysink")
    );
}

#[test]
fn restart_exe_path_leaves_a_normal_path() {
    let p = Path::new("/home/u/.local/bin/jellysink");
    assert_eq!(restart_exe_path(p), p);
}

#[test]
fn restart_exe_path_does_not_strip_unrelated_deleted_name() {
    let p = Path::new("/tmp/deleted");
    assert_eq!(restart_exe_path(p), p);
}

#[test]
fn restart_command_targets_the_given_path_not_current_exe() {
    let exe = Path::new("/opt/jellysink");
    let cmd = restart_command(exe, ["run", "--config", "/tmp/j"]);
    assert_eq!(cmd.get_program(), exe);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["run", "--config", "/tmp/j"]);
}

/// The asset list of a real release, in the order the release workflow
/// uploads it — every name carries the target, checksums included.
fn release_assets() -> Vec<ReleaseAsset> {
    [
        "jellysink-aarch64-unknown-linux-musl",
        "jellysink-aarch64-unknown-linux-musl.sha256",
        "jellysink-x86_64-unknown-linux-musl",
        "jellysink-x86_64-unknown-linux-musl.sha256",
        "jellytui-aarch64-unknown-linux-musl",
        "jellytui-aarch64-unknown-linux-musl.sha256",
        "jellytui-x86_64-unknown-linux-musl",
        "jellytui-x86_64-unknown-linux-musl.sha256",
    ]
    .iter()
    .map(|name| ReleaseAsset::new(*name, format!("https://host/{name}")))
    .collect()
}

#[test]
fn each_binary_gets_its_own_asset_and_never_a_checksum() {
    for bin in ["jellysink", "jellytui"] {
        let asset = match_asset(&release_assets(), bin, "x86_64-unknown-linux-musl").unwrap();
        assert_eq!(asset.name(), format!("{bin}-x86_64-unknown-linux-musl"));
    }
}

/// The ordering that matters: in upload order a substring match happens to
/// pick the daemon anyway, so only this one fails if the matcher is dropped.
#[test]
fn a_release_listing_the_other_binary_first_still_picks_the_asked_for_one() {
    let mut assets = release_assets();
    assets.reverse();
    let asset = match_asset(&assets, "jellysink", "x86_64-unknown-linux-musl").unwrap();
    assert_eq!(asset.name(), "jellysink-x86_64-unknown-linux-musl");
}

#[test]
fn an_arch_the_release_has_no_binary_for_matches_nothing() {
    // Better than silently installing the wrong architecture's binary.
    assert!(
        match_asset(
            &release_assets(),
            "jellysink",
            "riscv64gc-unknown-linux-musl"
        )
        .is_none()
    );
}

/// The frontend's baseline is its own version, not the running daemon's.
#[test]
fn the_installed_version_is_read_from_the_binary_itself() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("jellytui");
    fs::write(&path, b"#!/bin/sh\necho 'jellytui 0.9.0'\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(installed_version(&path).as_deref(), Some("0.9.0"));
}

#[test]
fn a_binary_that_cannot_be_asked_reports_no_version_rather_than_a_wrong_one() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("jellytui");
    fs::write(&path, b"not a program").unwrap();
    assert_eq!(installed_version(&path), None);
}
