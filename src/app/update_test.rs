use super::*;
use std::path::{Path, PathBuf};

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
