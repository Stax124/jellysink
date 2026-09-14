//! GitHub-release self-update: check, download, and replace a released binary.

use color_eyre::eyre::{WrapErr, eyre};
use self_update::ReleaseAsset;
use self_update::backends::github;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO_OWNER: &str = "Stax124";
const REPO_NAME: &str = "jellysink";

pub const JELLYSINK_BIN: &str = "jellysink";
pub const JELLYTUI_BIN: &str = "jellytui";

fn release_target() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

/// The asset holding `bin_name`. `self_update`'s default is a substring match
/// on the target, which every asset of the release carries.
fn match_asset(assets: &[ReleaseAsset], bin_name: &str, target: &str) -> Option<ReleaseAsset> {
    let wanted = format!("{bin_name}-{target}");
    assets.iter().find(|asset| asset.name() == wanted).cloned()
}

/// `progress` is the download bar only. `self_update`'s own commentary stays
/// off: its "*NOT* compatible" line fires on every 0.x minor bump.
fn updater(
    bin_name: &str,
    dest: Option<&Path>,
    current: &str,
    progress: bool,
) -> color_eyre::Result<github::AsyncUpdate> {
    let target = release_target().ok_or_else(|| {
        eyre!(
            "no GitHub release binary for arch {}",
            std::env::consts::ARCH
        )
    })?;
    let bin_name = bin_name.to_string();
    let mut builder = github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(&bin_name)
        .target(target)
        .current_version(current)
        .no_confirm(true)
        .show_output(false)
        .show_download_progress(progress)
        .check_install_path_writable(true)
        .asset_matcher(move |assets| match_asset(assets, &bin_name, target));
    if let Some(dest) = dest {
        builder.bin_install_path(dest);
    }
    builder.build_async().wrap_err("configuring GitHub updater")
}

pub async fn check(bin_name: &str) -> color_eyre::Result<Option<String>> {
    Ok(updater(bin_name, None, crate::VERSION, false)?
        .is_update_available_async()
        .await
        .wrap_err("checking GitHub releases")?
        .map(|release| release.version().to_string()))
}

/// `dest` of `None` replaces the running executable; `Some` one beside it,
/// whose own `--version` is then the baseline. `Ok(None)` means already current.
///
/// Safe on a binary that is running: the install renames over the path, so that
/// process keeps the inode it mapped.
pub async fn install(
    bin_name: &str,
    dest: Option<&Path>,
    progress: bool,
) -> color_eyre::Result<Option<String>> {
    let current = match dest {
        Some(path) => installed_version(path).unwrap_or_else(|| "0.0.0".to_string()),
        None => crate::VERSION.to_string(),
    };
    let status = updater(bin_name, dest, &current, progress)?
        .update_async()
        .await
        .wrap_err_with(|| format!("installing {bin_name} from GitHub releases"))?;
    if !status.is_updated() {
        return Ok(None);
    }
    if let Some(dest) = dest {
        // A bare-binary asset extracts 0644; only `self_replace` restores the mode.
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))
            .wrap_err_with(|| format!("making {} executable", dest.display()))?;
    }
    Ok(Some(status.version().to_string()))
}

/// `None` when it cannot be asked, leaving the caller to replace it rather than
/// trust a version it does not have.
fn installed_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;
    let line = String::from_utf8(output.stdout).ok()?;
    Some(line.split_whitespace().next_back()?.to_string())
}

pub fn sibling_binary(name: &str) -> Option<PathBuf> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        // Distinct from finding nothing beside us, which is a normal install.
        Err(e) => {
            tracing::warn!("could not resolve the running executable: {e}");
            return None;
        }
    };
    let path = exe.parent()?.join(name);
    path.is_file().then_some(path)
}

/// Linux `readlink(/proc/self/exe)` appends this after the original inode is unlinked.
const DELETED_SUFFIX: &str = " (deleted)";

/// Path to exec after `self_replace`; `current_exe()` is then `$path (deleted)`
/// and `execve` fails with ENOENT.
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
#[path = "update_test.rs"]
mod tests;
