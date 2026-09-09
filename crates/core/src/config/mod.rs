//! config.toml, cred.json and mpv_args.conf: where they live, how they parse,
//! and how they are written.

mod credentials;
mod mpv_args;
mod paths;
mod server;
mod settings;

pub use credentials::Credentials;
pub use mpv_args::MpvArgs;
pub use paths::Paths;
pub use server::{device_name, normalize_server_url};
pub use settings::{Config, Field};

use color_eyre::eyre::WrapErr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn atomic_write(path: &Path, data: &[u8], mode: u32) -> color_eyre::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f =
            fs::File::create(&tmp).wrap_err_with(|| format!("creating {}", tmp.display()))?;
        f.write_all(data)
            .wrap_err_with(|| format!("writing {}", tmp.display()))?;
        f.sync_all()
            .wrap_err_with(|| format!("flushing {}", tmp.display()))?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
        .wrap_err_with(|| format!("restricting {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .wrap_err_with(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
