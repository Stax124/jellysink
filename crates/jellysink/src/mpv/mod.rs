//! The mpv process and its IPC socket. Never sets `vo`, `hwdec`, `scale` or
//! `glsl-shaders`, and never passes `--no-config`.

mod command;
mod event;
mod ipc;

pub(crate) use event::{EndFileReason, MpvEvent, SelectedTrack};

use crate::runtime::task::AbortOnDrop;
use color_eyre::eyre::{WrapErr, eyre};
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

type Reply = oneshot::Sender<Result<Value, String>>;

/// Drops pending requests whose caller has gone away (timed out or cancelled).
/// mpv never replies to those, so they would leak a `oneshot::Sender` each.
fn evict_abandoned(pending: &mut HashMap<i64, Reply>) -> usize {
    let before = pending.len();
    pending.retain(|_, reply| !reply.is_closed());
    before - pending.len()
}

struct Request {
    line: String,
    id: i64,
    reply: Reply,
}

/// One mpv process. Its events live and die with it, so dropping the session
/// also drops whatever it had queued.
pub(crate) struct MpvSession {
    child: Child,
    requests: mpsc::UnboundedSender<Request>,
    events: mpsc::UnboundedReceiver<MpvEvent>,
    _ipc: AbortOnDrop,
    socket: PathBuf,
    next_id: i64,
}

impl MpvSession {
    pub(crate) async fn spawn(
        mpv_path: &str,
        extra_args: &[String],
        socket: PathBuf,
    ) -> color_eyre::Result<Self> {
        if let Some(parent) = socket.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .wrap_err_with(|| format!("creating {}", parent.display()))?;
        }
        if let Err(e) = tokio::fs::remove_file(&socket).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("could not remove stale {}: {e}", socket.display());
        }

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
            .wrap_err_with(|| format!("waiting for mpv IPC socket {}", socket.display()))?;
        // mpv creates the socket under the ambient umask, and its
        // `http-header-fields` carries the Jellyfin access token.
        if let Err(e) =
            tokio::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).await
        {
            tracing::warn!("could not restrict {}: {e}", socket.display());
        }

        let (requests, request_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let ipc = AbortOnDrop(tokio::spawn(ipc_loop(stream, request_rx, event_tx)));

        Ok(Self {
            child,
            requests,
            events,
            _ipc: ipc,
            socket,
            next_id: 1,
        })
    }

    /// A closed channel means the IPC loop ended, which only a dead mpv causes.
    pub(crate) async fn next_event(&mut self) -> MpvEvent {
        self.events.recv().await.unwrap_or(MpvEvent::Exited)
    }

    fn next_request_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn command(&mut self, args: Vec<Value>) -> color_eyre::Result<Value> {
        let command = command_label(&args);
        let id = self.next_request_id();
        let line = encode_command(id, &args);
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request { line, id, reply })
            .map_err(|_| eyre!("mpv {command}: IPC on {} closed", self.socket.display()))?;
        match timeout(Duration::from_secs(10), answer).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(eyre!("mpv {command}: {e}")),
            Ok(Err(_)) => Err(eyre!("mpv {command}: dropped")),
            Err(_) => Err(eyre!("mpv {command}: timed out")),
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

    /// IPC `quit`, then `SIGTERM`, then `SIGKILL`; `Drop` removes the socket.
    pub(crate) async fn quit(mut self) {
        // mpv may close the socket before answering; the escalation covers a real failure.
        let _ = self.command(vec![json!("quit")]).await;
        if timeout(Duration::from_secs(3), self.child.wait())
            .await
            .is_ok()
        {
            return;
        }
        if let Some(id) = self.child.id()
            && let Some(pid) = rustix::process::Pid::from_raw(id as i32)
            && let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::TERM)
        {
            tracing::warn!("could not SIGTERM mpv ({id}): {e}");
        }
        if timeout(Duration::from_secs(2), self.child.wait())
            .await
            .is_err()
            && let Err(e) = self.child.kill().await
        {
            tracing::warn!("could not kill mpv: {e}");
        }
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// The command and, for a property access, the property. Never a later
/// argument: a `loadfile` URL can carry the access token.
fn command_label(args: &[Value]) -> String {
    let name = args.first().and_then(Value::as_str).unwrap_or("command");
    match args.get(1).and_then(Value::as_str) {
        Some(property) if name.ends_with("_property") => format!("{name} {property}"),
        _ => name.to_string(),
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
    mut requests: mpsc::UnboundedReceiver<Request>,
    events: mpsc::UnboundedSender<MpvEvent>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let mut pending: HashMap<i64, Reply> = HashMap::new();

    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(Request { line, id, reply }) = request else {
                    break;
                };
                let evicted = evict_abandoned(&mut pending);
                if evicted > 0 {
                    tracing::debug!(evicted, "dropped mpv IPC requests the caller gave up on");
                }
                pending.insert(id, reply);
                if let Err(e) = writer.write_all(line.as_bytes()).await {
                    tracing::warn!("mpv IPC write failed: {e}");
                    break;
                }
            }
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("mpv IPC read failed: {e}");
                        break;
                    }
                };
                match parse_ipc_line(&line) {
                    Ok(IpcMessage::Reply { request_id, result }) => {
                        if let Some(reply) = pending.remove(&request_id) {
                            let _ = reply.send(result);
                        }
                    }
                    Ok(IpcMessage::Event(Some(event))) => {
                        if events.send(event).is_err() {
                            break;
                        }
                    }
                    Ok(IpcMessage::Event(None)) => {}
                    Err(e) => tracing::warn!("unparseable mpv IPC line: {e:#}"),
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
