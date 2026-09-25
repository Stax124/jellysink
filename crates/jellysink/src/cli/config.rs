use jellysink_core::config::{Config, Field, Paths};

pub(crate) fn cmd_config_path(paths: &Paths) -> color_eyre::Result<()> {
    println!("{}", paths.config_dir.display());
    Ok(())
}

pub(crate) fn cmd_config_get(paths: &Paths, key: Option<&str>) -> color_eyre::Result<()> {
    let Some(key) = key else {
        let config = Config::load(paths)?;
        print!("{}", config.to_toml()?);
        let args = Field::MpvArgs.read(paths)?;
        if !args.trim().is_empty() {
            println!("\n# {}", Field::MpvArgs.name());
            println!("{}", args.trim_end());
        }
        return Ok(());
    };
    println!("{}", Field::parse(key)?.read(paths)?.trim_end());
    Ok(())
}

pub(crate) fn cmd_config_set(paths: &Paths, key: &str, value: &str) -> color_eyre::Result<()> {
    Field::parse(key)?.write(paths, value)
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
