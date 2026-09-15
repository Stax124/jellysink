use jellysink_core::config::{Config, Field, MpvArgs, Paths};

pub(crate) fn cmd_config_path(paths: &Paths) -> color_eyre::Result<()> {
    println!("{}", paths.config_dir.display());
    Ok(())
}

pub(crate) fn cmd_config_get(paths: &Paths, key: Option<&str>) -> color_eyre::Result<()> {
    let Some(key) = key else {
        let config = Config::load(paths)?;
        print!("{}", config.to_toml()?);
        let args = MpvArgs::get(paths)?;
        if !args.trim().is_empty() {
            println!("\n# {}", Field::MpvArgs.name());
            println!("{}", args.trim_end());
        }
        return Ok(());
    };
    let field = Field::parse(key)?;
    let out = match Config::load(paths)?.get(field) {
        Some(value) => value,
        // Not in config.toml; a running daemon re-reads it on every mpv spawn.
        None => MpvArgs::get(paths)?,
    };
    println!("{}", out.trim_end());
    Ok(())
}

pub(crate) fn cmd_config_set(paths: &Paths, key: &str, value: &str) -> color_eyre::Result<()> {
    let field = Field::parse(key)?;
    let mut config = Config::load(paths)?;
    if config.set(field, value)? {
        config.save(paths)?;
    } else {
        MpvArgs::save(paths, value)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
