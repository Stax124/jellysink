//! Open a command in the user's terminal emulator (tray update progress).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

const APP_TITLE: &str = "jellysink";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalLaunch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
}

/// Fallbacks after `xdg-terminal-exec` and `$TERMINAL`. Prefix is the exec flag(s).
const FALLBACKS: &[(&str, &[&str])] = &[
    ("kitty", &["-e"]),
    ("ghostty", &["-e"]),
    ("alacritty", &["-e"]),
    ("foot", &["-e"]),
    ("xterm", &["-e"]),
    ("konsole", &["-e"]),
    ("tilix", &["-e"]),
    ("wezterm", &["start", "--"]),
    ("gnome-terminal", &["--"]),
    ("kgx", &["--"]),
    ("ptyxis", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("mate-terminal", &["-x"]),
];

fn exec_prefix(basename: &str) -> &'static [&'static str] {
    FALLBACKS
        .iter()
        .find(|(name, _)| *name == basename)
        .map(|(_, prefix)| *prefix)
        .unwrap_or(&["-e"])
}

pub(crate) fn terminal_candidates(
    argv: &[impl AsRef<OsStr>],
    available: &dyn Fn(&str) -> Option<PathBuf>,
    env_terminal: Option<&OsStr>,
) -> Vec<TerminalLaunch> {
    let payload: Vec<OsString> = argv.iter().map(|a| a.as_ref().to_os_string()).collect();
    let mut out = Vec::new();

    if let Some(program) = available("xdg-terminal-exec") {
        let mut args = vec![
            OsString::from(format!("--title={APP_TITLE}")),
            OsString::from("--"),
        ];
        args.extend(payload.iter().cloned());
        out.push(TerminalLaunch { program, args });
    }

    if let Some(term) = env_terminal {
        let lookup = term.to_str().unwrap_or("");
        let by_name = Path::new(term)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if let Some(program) = available(lookup).or_else(|| {
            if by_name != lookup {
                available(by_name)
            } else {
                None
            }
        }) {
            let mut args: Vec<OsString> = exec_prefix(by_name)
                .iter()
                .map(|s| OsString::from(*s))
                .collect();
            args.extend(payload.iter().cloned());
            out.push(TerminalLaunch { program, args });
        }
    }

    for (name, prefix) in FALLBACKS {
        if let Some(program) = available(name) {
            let mut args: Vec<OsString> = prefix.iter().map(|s| OsString::from(*s)).collect();
            args.extend(payload.iter().cloned());
            out.push(TerminalLaunch { program, args });
        }
    }
    out
}

pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

const SPAWN_PROBE: Duration = Duration::from_millis(150);

/// Spawn `argv` inside a terminal window. The daemon does not wait on it.
pub(crate) async fn spawn_in_terminal(argv: &[impl AsRef<OsStr>]) -> std::io::Result<()> {
    spawn_launches(&terminal_candidates(
        argv,
        &find_on_path,
        std::env::var_os("TERMINAL").as_deref(),
    ))
    .await
}

async fn spawn_launches(launches: &[TerminalLaunch]) -> std::io::Result<()> {
    if launches.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no terminal emulator found",
        ));
    }
    let mut last_err =
        std::io::Error::new(std::io::ErrorKind::NotFound, "no terminal emulator found");
    for launch in launches {
        // tokio's Command, not std's: this runs on the daemon's
        // `current_thread` runtime, where a synchronous fork/exec stalls the
        // WebSocket keepalive and mpv IPC along with everything else.
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        match cmd.spawn() {
            Ok(mut child) => match spawn_looks_ok(&mut child, SPAWN_PROBE).await {
                Ok(()) => return Ok(()),
                Err(e) => last_err = e,
            },
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Ok if the child is still running or exited 0 (double-fork). Err if it
/// exited non-zero before `timeout`.
///
/// Awaits the child rather than polling `try_wait` every 20 ms: with up to
/// fifteen candidate terminals, that loop could spend seconds of the daemon's
/// single runtime thread doing nothing.
async fn spawn_looks_ok(child: &mut Child, timeout: Duration) -> std::io::Result<()> {
    match tokio::time::timeout(timeout, child.wait()).await {
        // Still running when the probe expired: it launched.
        Err(_) => Ok(()),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(std::io::Error::other(format!(
            "terminal exited with {status}"
        ))),
        Ok(Err(e)) => Err(e),
    }
}

#[cfg(test)]
#[path = "terminal_test.rs"]
mod tests;
