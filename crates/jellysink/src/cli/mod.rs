//! The `jellysink` subcommands. Output here is `println!`: these run in front
//! of a user, not under the daemon's tracing subscriber.

mod auth;
mod config;
mod control;
mod run;
mod update;

pub(crate) use auth::{cmd_login, cmd_logout};
pub(crate) use config::{cmd_config_get, cmd_config_path, cmd_config_set};
pub(crate) use control::{cmd_status, cmd_stop};
pub(crate) use run::cmd_run;
pub(crate) use update::cmd_update;
