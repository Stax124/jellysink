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
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;

    fn payload() -> Vec<OsString> {
        vec![
            "/tmp/jellysink".into(),
            "update".into(),
            "--from-tray".into(),
        ]
    }

    fn available_from(names: &[&str]) -> impl Fn(&str) -> Option<PathBuf> {
        let map: HashMap<String, PathBuf> = names
            .iter()
            .map(|n| (n.to_string(), PathBuf::from(format!("/usr/bin/{n}"))))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn args_as_str(launch: &TerminalLaunch) -> Vec<String> {
        launch
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn xdg_terminal_exec_wins_when_present() {
        let available = available_from(&["xdg-terminal-exec", "kitty"]);
        let launches = terminal_candidates(&payload(), &available, None);
        assert_eq!(
            launches[0].program,
            PathBuf::from("/usr/bin/xdg-terminal-exec")
        );
        assert_eq!(
            args_as_str(&launches[0]),
            vec![
                "--title=jellysink",
                "--",
                "/tmp/jellysink",
                "update",
                "--from-tray"
            ]
        );
    }

    #[test]
    fn env_terminal_used_when_no_xdg() {
        let available = available_from(&["kitty"]);
        let launches = terminal_candidates(&payload(), &available, Some(OsStr::new("kitty")));
        assert_eq!(launches[0].program, PathBuf::from("/usr/bin/kitty"));
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["-e", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn wezterm_uses_start_dash_dash() {
        let available = available_from(&["wezterm"]);
        let launches = terminal_candidates(&payload(), &available, None);
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["start", "--", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn gnome_terminal_uses_double_dash() {
        let available = available_from(&["gnome-terminal"]);
        let launches = terminal_candidates(&payload(), &available, None);
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["--", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn xfce4_terminal_uses_dash_x() {
        let available = available_from(&["xfce4-terminal"]);
        let launches = terminal_candidates(&payload(), &available, None);
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["-x", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn kitty_uses_dash_e() {
        let available = available_from(&["kitty"]);
        let launches = terminal_candidates(&payload(), &available, None);
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["-e", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn nothing_available_yields_empty() {
        let available = available_from(&[]);
        let launches = terminal_candidates(&payload(), &available, Some(OsStr::new("kitty")));
        assert!(launches.is_empty());
    }

    #[test]
    fn missing_env_terminal_falls_through_to_fallback() {
        let available = available_from(&["foot"]);
        let launches = terminal_candidates(&payload(), &available, Some(OsStr::new("kitty")));
        assert_eq!(launches[0].program, PathBuf::from("/usr/bin/foot"));
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["-e", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn env_terminal_wezterm_uses_start_dash_dash() {
        let available = available_from(&["wezterm"]);
        let launches = terminal_candidates(&payload(), &available, Some(OsStr::new("wezterm")));
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["start", "--", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn env_terminal_absolute_gnome_uses_double_dash() {
        let available = available_from(&["gnome-terminal"]);
        let launches = terminal_candidates(
            &payload(),
            &available,
            Some(OsStr::new("/usr/bin/gnome-terminal")),
        );
        assert_eq!(
            launches[0].program,
            PathBuf::from("/usr/bin/gnome-terminal")
        );
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["--", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[test]
    fn env_terminal_unknown_defaults_to_dash_e() {
        let available = available_from(&["myterm"]);
        let launches = terminal_candidates(&payload(), &available, Some(OsStr::new("myterm")));
        assert_eq!(
            args_as_str(&launches[0]),
            vec!["-e", "/tmp/jellysink", "update", "--from-tray"]
        );
    }

    #[tokio::test]
    async fn spawn_looks_ok_rejects_immediate_nonzero_exit() {
        let mut child = Command::new("false").spawn().unwrap();
        let err = spawn_looks_ok(&mut child, Duration::from_millis(200))
            .await
            .expect_err("nonzero exit should be a spawn failure");
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
    }

    #[tokio::test]
    async fn spawn_looks_ok_accepts_immediate_zero_exit() {
        let mut child = Command::new("true").spawn().unwrap();
        spawn_looks_ok(&mut child, Duration::from_millis(200))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn spawn_looks_ok_accepts_still_running() {
        let mut child = Command::new("sleep").arg("10").spawn().unwrap();
        let result = spawn_looks_ok(&mut child, Duration::from_millis(80)).await;
        let _ = child.kill().await;
        result.unwrap();
    }

    #[tokio::test]
    async fn spawn_launches_skips_immediate_failure() {
        let launches = vec![
            TerminalLaunch {
                program: PathBuf::from("false"),
                args: Vec::new(),
            },
            TerminalLaunch {
                program: PathBuf::from("true"),
                args: Vec::new(),
            },
        ];
        spawn_launches(&launches).await.unwrap();
    }

    #[tokio::test]
    async fn spawn_launches_fails_when_every_candidate_exits_nonzero() {
        let launches = vec![TerminalLaunch {
            program: PathBuf::from("false"),
            args: Vec::new(),
        }];
        let err = spawn_launches(&launches).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
    }
}
