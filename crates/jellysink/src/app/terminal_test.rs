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
