use jellysink_core::config::{Config, Field, MpvArgs, Paths};

pub(crate) fn cmd_config_path(paths: &Paths) -> color_eyre::Result<()> {
    println!("{}", paths.config_dir.display());
    Ok(())
}

pub(crate) fn cmd_config_get(paths: &Paths, key: Option<&str>) -> color_eyre::Result<()> {
    let Some(key) = key else {
        let cfg = Config::load(paths)?;
        print!("{}", cfg.to_toml()?);
        let args = MpvArgs::get(paths)?;
        if !args.trim().is_empty() {
            println!("\n# {}", Field::MpvArgs.name());
            println!("{}", args.trim_end());
        }
        return Ok(());
    };
    let field = Field::parse(key)?;
    let out = match Config::load(paths)?.get(field) {
        Some(v) => v,
        // Not in config.toml; a running daemon re-reads it on every mpv spawn.
        None => MpvArgs::get(paths)?,
    };
    println!("{}", out.trim_end());
    Ok(())
}

pub(crate) fn cmd_config_set(paths: &Paths, key: &str, value: &str) -> color_eyre::Result<()> {
    let field = Field::parse(key)?;
    let mut cfg = Config::load(paths)?;
    if cfg.set(field, value)? {
        cfg.save(paths)?;
    } else {
        MpvArgs::save(paths, value)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
