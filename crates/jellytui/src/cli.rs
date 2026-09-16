//! `jellytui update`. Output here is `println!`: it runs in front of a user on
//! the normal screen, never behind the alternate one.

use color_eyre::eyre::{Result, WrapErr};
use jellysink_core::config::Paths;
use jellysink_core::update::{exec_updated, install_both, print_check, restart_exe_path};

const BIN_NAME: &str = env!("CARGO_BIN_NAME");

pub(crate) async fn cmd_update(paths: &Paths, check_only: bool, force: bool) -> Result<()> {
    if check_only {
        return print_check(BIN_NAME).await;
    }
    install_both(paths, BIN_NAME, force).await
}

/// For the `u` key: the user asked from inside the terminal UI and expects it
/// back, so the new binary takes this process over.
pub(crate) async fn update_and_restart(paths: &Paths) -> Result<()> {
    cmd_update(paths, false, false).await?;
    println!("Restarting...");
    let exe = restart_exe_path(&std::env::current_exe().wrap_err("resolving current executable")?);
    Err(exec_updated(&exe)).wrap_err_with(|| format!("restarting {}", exe.display()))
}
