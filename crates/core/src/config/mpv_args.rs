use super::{Paths, atomic_write, read_optional};

/// Extra mpv argv, re-read on every spawn. Whitespace-separated; blank lines
/// and `#` comments are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MpvArgs(pub Vec<String>);

impl MpvArgs {
    pub fn load(paths: &Paths) -> color_eyre::Result<Self> {
        let text = read_optional(&paths.mpv_args_file())?.unwrap_or_default();
        Ok(Self(parse_mpv_args(&text)))
    }

    pub(super) fn save(paths: &Paths, value: &str) -> color_eyre::Result<()> {
        let args = parse_mpv_args(value);
        let mut text = String::new();
        for arg in &args {
            text.push_str(arg);
            text.push('\n');
        }
        paths.ensure()?;
        atomic_write(&paths.mpv_args_file(), text.as_bytes(), 0o644)
    }
}

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
