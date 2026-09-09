//! The mpv process and its IPC socket. Never sets `vo`, `hwdec`, `scale` or
//! `glsl-shaders`, and never passes `--no-config`: the user's own mpv config
//! and upscalers are the point.

mod command;
mod event;
mod ipc;

pub(crate) use event::{EndFileReason, MpvEvent, SelectedTrack};

use color_eyre::eyre::{WrapErr, eyre};
use event::mpv_event_for;
use ipc::{
    IpcMessage, as_bool_property, as_f64_property, as_i64_property, encode_command, parse_ipc_line,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

struct Pending {
    tx: oneshot::Sender<Result<Value, String>>,
}

/// Drops pending requests whose caller has gone away (timed out or cancelled).
/// mpv never replies to those, so they would leak a `oneshot::Sender` each.
fn evict_abandoned(pending: &mut HashMap<i64, Pending>) -> usize {
    let before = pending.len();
    pending.retain(|_, p| !p.tx.is_closed());
    before - pending.len()
}

pub(crate) struct MpvSession {
    child: Child,
    cmd_tx: mpsc::UnboundedSender<IpcCmd>,
    socket: PathBuf,
    next_id: i64,
}

enum IpcCmd {
    Request {
        line: String,
        id: i64,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Shutdown,
}

impl MpvSession {
    pub(crate) async fn spawn(
        mpv_path: &str,
        extra_args: &[String],
        socket: PathBuf,
    ) -> color_eyre::Result<(Self, mpsc::UnboundedReceiver<MpvEvent>)> {
        if let Some(parent) = socket.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .wrap_err("creating mpv socket dir")?;
        }
        let _ = tokio::fs::remove_file(&socket).await;

        let mut cmd = Command::new(mpv_path);
        cmd.arg(format!("--input-ipc-server={}", socket.display()))
            .arg("--force-window=yes")
            .arg("--idle=yes")
            .args(extra_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let child = cmd
            .spawn()
            .wrap_err_with(|| format!("spawning {mpv_path} (is mpv installed?)"))?;

        let stream = wait_for_socket(&socket, Duration::from_secs(8))
            .await
            .wrap_err("waiting for mpv IPC socket")?;
        // mpv creates the socket under the ambient umask, and its
        // `http-header-fields` carries the Jellyfin access token.
        if let Err(e) =
            tokio::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).await
        {
            tracing::warn!("could not restrict {}: {e}", socket.display());
        }

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(ipc_loop(stream, cmd_rx, ev_tx));

        Ok((
            Self {
                child,
                cmd_tx,
                socket,
                next_id: 1,
            },
            ev_rx,
        ))
    }

    fn next_request_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn command(&mut self, args: Vec<Value>) -> color_eyre::Result<Value> {
        let id = self.next_request_id();
        let line = encode_command(id, &args);
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(IpcCmd::Request {
                line,
                id,
                reply: tx,
            })
            .map_err(|_| eyre!("mpv IPC closed"))?;
        match timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(eyre!("mpv command error: {e}")),
            Ok(Err(_)) => Err(eyre!("mpv command dropped")),
            Err(_) => Err(eyre!("mpv command timed out")),
        }
    }

    pub(crate) async fn set_property(
        &mut self,
        name: &str,
        value: Value,
    ) -> color_eyre::Result<()> {
        self.command(vec![json!("set_property"), json!(name), value])
            .await?;
        Ok(())
    }

    async fn get_property(&mut self, name: &str) -> color_eyre::Result<Value> {
        self.command(vec![json!("get_property"), json!(name)]).await
    }

    async fn get_i64(&mut self, name: &str) -> color_eyre::Result<i64> {
        as_i64_property(name, &self.get_property(name).await?)
    }

    async fn get_f64(&mut self, name: &str) -> color_eyre::Result<f64> {
        as_f64_property(name, &self.get_property(name).await?)
    }

    async fn get_bool(&mut self, name: &str) -> color_eyre::Result<bool> {
        as_bool_property(name, &self.get_property(name).await?)
    }

    pub(crate) async fn quit_and_wait(&mut self) -> color_eyre::Result<()> {
        let _ = self.command(vec![json!("quit")]).await;
        let _ = self.cmd_tx.send(IpcCmd::Shutdown);
        if timeout(Duration::from_secs(3), self.child.wait())
            .await
            .is_ok()
        {
            let _ = tokio::fs::remove_file(&self.socket).await;
            return Ok(());
        }
        if let Some(id) = self.child.id()
            && let Some(pid) = rustix::process::Pid::from_raw(id as i32)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        match timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                let _ = self.child.kill().await;
                let _ = self.child.wait().await;
            }
        }
        let _ = tokio::fs::remove_file(&self.socket).await;
        Ok(())
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(IpcCmd::Shutdown);
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.socket);
    }
}

async fn wait_for_socket(path: &Path, max: Duration) -> color_eyre::Result<UnixStream> {
    let start = tokio::time::Instant::now();
    loop {
        match UnixStream::connect(path).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                if start.elapsed() > max {
                    return Err(eyre!("mpv socket never appeared: {e}"));
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn ipc_loop(
    stream: UnixStream,
    mut cmd_rx: mpsc::UnboundedReceiver<IpcCmd>,
    ev_tx: mpsc::UnboundedSender<MpvEvent>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let mut pending: HashMap<i64, Pending> = HashMap::new();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(IpcCmd::Request { line, id, reply }) => {
                        let evicted = evict_abandoned(&mut pending);
                        if evicted > 0 {
                            tracing::debug!(evicted, "dropped mpv IPC requests the caller gave up on");
                        }
                        pending.insert(id, Pending { tx: reply });
                        if writer.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Some(IpcCmd::Shutdown) | None => break,
                }
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        match parse_ipc_line(&line) {
                            Ok(IpcMessage::Reply { request_id, error, data }) => {
                                if let Some(p) = pending.remove(&request_id) {
                                    let r = if error == "success" {
                                        Ok(data)
                                    } else {
                                        Err(error)
                                    };
                                    let _ = p.tx.send(r);
                                }
                            }
                            Ok(other) => {
                                if let Some(ev) = mpv_event_for(&other)
                                    && ev_tx.send(ev).is_err()
                                {
                                    break;
                                }
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = ev_tx.send(MpvEvent::Exited);
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;

#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_tests;
