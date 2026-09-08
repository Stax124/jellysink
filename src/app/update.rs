//! GitHub-release self-update: check, download, and replace this binary.

use color_eyre::eyre::{WrapErr, eyre};
use self_update::backends::github;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const REPO_OWNER: &str = "Stax124";
pub(crate) const REPO_NAME: &str = "jellysink";
pub(crate) const BIN_NAME: &str = "jellysink";

pub(crate) fn release_target() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

pub(crate) struct UpdateOffer {
    pub(crate) version: String,
}

/// `progress` is the download bar only. `self_update`'s own commentary stays
/// off: its "*NOT* compatible" line fires on every 0.x minor bump.
fn updater(progress: bool) -> color_eyre::Result<github::AsyncUpdate> {
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
        .show_output(false)
        .show_download_progress(progress)
        .check_install_path_writable(true);
    builder.build_async().wrap_err("configuring GitHub updater")
}

pub(crate) async fn check() -> color_eyre::Result<Option<UpdateOffer>> {
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

pub(crate) async fn install(progress: bool) -> color_eyre::Result<self_update::VersionStatus> {
    updater(progress)?
        .update_async()
        .await
        .wrap_err("installing update from GitHub releases")
}

/// Linux `readlink(/proc/self/exe)` appends this after the original inode is unlinked.
const DELETED_SUFFIX: &str = " (deleted)";

/// Path to exec after `self_replace`; `current_exe()` is then `$path (deleted)`
/// and `execve` fails with ENOENT.
pub(crate) fn restart_exe_path(current: &Path) -> PathBuf {
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
pub(crate) fn exec_updated(exe: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    restart_command(exe, std::env::args_os().skip(1)).exec()
}

#[cfg(test)]
#[path = "update_test.rs"]
mod tests;
