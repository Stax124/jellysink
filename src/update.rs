//! GitHub-release self-update: check, download, and replace this binary.

use color_eyre::eyre::{WrapErr, eyre};
use self_update::backends::github;

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

#[cfg(test)]
mod tests {
    use super::release_target;

    #[test]
    fn release_target_is_linux_musl_for_this_arch() {
        let t = release_target().expect("jellysink only ships linux musl for x86_64/aarch64");
        assert!(t.ends_with("-unknown-linux-musl"), "{t}");
        assert!(t.starts_with(std::env::consts::ARCH), "{t}");
    }
}
