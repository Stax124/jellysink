use super::Paths;
use super::atomic_write;
use color_eyre::eyre::WrapErr;
use std::fs;

/// Extra mpv argv, kept in its own file and re-read on every spawn.
///
/// One argument per line (`--title=My Movie` is one line, not two words);
/// blank lines and `#` comments are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MpvArgs(pub Vec<String>);

impl MpvArgs {
    pub fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let path = paths.mpv_args_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        Ok(Self(parse_mpv_args(&text)))
    }

    pub fn save(paths: &Paths, value: &str) -> color_eyre::Result<()> {
        let args = parse_mpv_args(value);
        let mut text = String::new();
        for arg in &args {
            text.push_str(arg);
            text.push('\n');
        }
        paths.ensure()?;
        atomic_write(&paths.mpv_args_file(), text.as_bytes(), 0o644)?;
        Ok(())
    }

    pub fn get(paths: &Paths) -> color_eyre::Result<String> {
        Ok(Self::load(paths)?.0.join(" "))
    }
}

/// Split a value into mpv arguments. Whitespace-separated, like a shell
/// command line without quoting.
fn parse_mpv_args(value: &str) -> Vec<String> {
    value
        .lines()
        .flat_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                Vec::new()
            } else {
                line.split_whitespace().map(str::to_string).collect()
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "mpv_args_test.rs"]
mod tests;
