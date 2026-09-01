//! GitHub-release self-update: check, download, and replace this binary.

use color_eyre::eyre::{WrapErr, eyre};
use self_update::backends::github;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REPO_OWNER: &str = "Stax124";
pub const REPO_NAME: &str = "jellysink";
pub const BIN_NAME: &str = "jellysink";

pub fn release_target() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

pub struct UpdateOffer {
    pub version: String,
}

fn updater(show_output: bool) -> color_eyre::Result<github::AsyncUpdate> {
    let target = release_target().ok_or_else(|| {
        eyre!(
            "no GitHub release binary for arch {}",
            std::env::consts::ARCH
        )
    })?;
    let mut builder = github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(target)
        .current_version(env!("CARGO_PKG_VERSION"))
        .no_confirm(true)
        .show_output(show_output)
        .show_download_progress(show_output)
        .check_install_path_writable(true);
    builder.build_async().wrap_err("configuring GitHub updater")
}

pub async fn check() -> color_eyre::Result<Option<UpdateOffer>> {
    match updater(false)?
        .is_update_available_async()
        .await
        .wrap_err("checking GitHub releases")?
    {
        Some(release) => Ok(Some(UpdateOffer {
            version: release.version().to_string(),
        })),
        None => Ok(None),
    }
}

pub async fn install(show_output: bool) -> color_eyre::Result<self_update::VersionStatus> {
    updater(show_output)?
        .update_async()
        .await
        .wrap_err("installing update from GitHub releases")
}

/// Linux `readlink(/proc/self/exe)` appends this after the original inode is unlinked.
const DELETED_SUFFIX: &str = " (deleted)";

/// Directory-entry path to exec after `self_replace`. `current_exe()` at restart
/// time is `$path (deleted)` and `execve` fails with ENOENT.
pub fn restart_exe_path(current: &Path) -> PathBuf {
    match current.to_str() {
        Some(s) if s.ends_with(DELETED_SUFFIX) => {
            PathBuf::from(&s[..s.len() - DELETED_SUFFIX.len()])
        }
        _ => current.to_path_buf(),
    }
}

fn restart_command(exe: &Path, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    cmd
}

/// Replace this process with `exe`. Returns only on failure.
pub fn exec_updated(exe: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    restart_command(exe, std::env::args_os().skip(1)).exec()
}

#[cfg(test)]
mod tests {
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
}
