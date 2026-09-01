use color_eyre::eyre::{WrapErr, eyre};
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

/// Inbound IPC message from mpv
#[derive(Debug, Clone, PartialEq)]
pub enum IpcMessage {
    Reply {
        request_id: i64,
        error: String,
        data: Value,
    },
    Event {
        name: String,
        reason: Option<String>,
        data: Value,
    },
}

/// Encodes a command to be sent to mpv
pub fn encode_command(request_id: i64, args: &[Value]) -> String {
    let v = json!({
        "command": args,
        "request_id": request_id,
    });
    format!("{v}\n")
}

/// M3U with one `#EXTINF` entry per `(title, url)`.
///
/// `loadfile` `force-media-title` and `playlist/N/title` do not populate
/// unloaded entries — this is what gives the selector its titles.
pub fn playlist_m3u<I, T, U>(entries: I) -> String
where
    I: IntoIterator<Item = (T, U)>,
    T: AsRef<str>,
    U: AsRef<str>,
{
    let mut body = String::from("#EXTM3U\n");
    for (title, url) in entries {
        let title = title.as_ref().replace(['\r', '\n'], " ");
        body.push_str("#EXTINF:-1,");
        body.push_str(&title);
        body.push('\n');
        body.push_str(url.as_ref());
        body.push('\n');
    }
    body
}

/// Args for `loadlist` `append` command to add a file to the playlist
pub fn loadlist_append_args(path: &str) -> [Value; 3] {
    [json!("loadlist"), json!(path), json!("append")]
}

/// Args for `loadlist` `insert-at` to splice entries in at `index`.
///
/// `insert-at` and the index are separate arguments; `"insert-at0"` as a single
/// token is `invalid parameter`. Inserting at or below the current position
/// does not interrupt playback — mpv shifts `playlist-pos` by the number
/// inserted and keeps playing the same file.
pub fn loadlist_insert_at_args(path: &str, index: usize) -> [Value; 4] {
    [
        json!("loadlist"),
        json!(path),
        json!("insert-at"),
        json!(index),
    ]
}

/// `yes` pauses only on the last playlist entry and auto-plays the rest,
/// which is also what emits `end-file` so we can adopt the new item.
/// `always` pauses on the last frame of every file without unloading it,
/// so we never see `end-file` and autoplay stalls.
pub const KEEP_OPEN: &str = "yes";

/// Parses an IPC line from mpv into an [`IpcMessage`]
pub fn parse_ipc_line(line: &str) -> color_eyre::Result<IpcMessage> {
    let v: Value = serde_json::from_str(line.trim()).wrap_err("mpv IPC JSON")?;
    if let Some(name) = v.get("event").and_then(Value::as_str) {
        let reason = v.get("reason").and_then(Value::as_str).map(str::to_string);
        return Ok(IpcMessage::Event {
            name: name.to_string(),
            reason,
            data: v,
        });
    }
    let request_id = v
        .get("request_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| eyre!("IPC reply missing request_id"))?;
    let error = v
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("success")
        .to_string();
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    Ok(IpcMessage::Reply {
        request_id,
        error,
        data,
    })
}

/// Converts a JSON value to a `f64` representing seconds
pub fn json_as_seconds(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_u64().map(|n| n as f64))
}

/// Takes the latest sub title ID from a track list (usually external subtitles not burned into the video)
pub fn max_sub_sid_from_track_list(list: &Value) -> i64 {
    let mut max = 0i64;
    if let Some(arr) = list.as_array() {
        for t in arr {
            if t.get("type").and_then(Value::as_str) == Some("sub")
                && let Some(id) = t.get("id").and_then(Value::as_i64)
            {
                max = max.max(id);
            }
        }
    }
    max
}

/// Represents an event received from mpv
#[derive(Debug, Clone)]
pub enum MpvEvent {
    EndFile { reason: String },
    FileLoaded,
    Exited,
}

struct Pending {
    tx: oneshot::Sender<Result<Value, String>>,
}

/// Represents a session with an mpv process
pub struct MpvSession {
    child: Child,
    cmd_tx: mpsc::UnboundedSender<IpcCmd>,
    socket: PathBuf,
    next_id: i64,
}

/// Represents a command to be sent to the mpv process
enum IpcCmd {
    Request {
        line: String,
        id: i64,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Shutdown,
}

impl MpvSession {
    /// Spawns a new mpv session with the given path and arguments
    pub async fn spawn(
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

    /// Sends a command to the mpv process and returns the response
    pub async fn command(&mut self, args: Vec<Value>) -> color_eyre::Result<Value> {
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

    pub async fn set_property(&mut self, name: &str, value: Value) -> color_eyre::Result<()> {
        self.command(vec![json!("set_property"), json!(name), value])
            .await?;
        Ok(())
    }

    pub async fn get_property(&mut self, name: &str) -> color_eyre::Result<Value> {
        self.command(vec![json!("get_property"), json!(name)]).await
    }

    pub async fn loadfile(&mut self, url: &str, title: Option<&str>) -> color_eyre::Result<()> {
        // Do not pass options as loadfile's 4th argument. Since mpv 0.38 that
        // slot is an insert *index* (integer); a map there is "invalid parameter"
        // and the file never loads. Set force-media-title as a property instead,
        // which is what jellyfin-mpv-shim does.
        if let Some(title) = title {
            let _ = self.set_property("force-media-title", json!(title)).await;
        }
        self.command(vec![json!("loadfile"), json!(url), json!("replace")])
            .await?;
        Ok(())
    }

    /// Appends every entry in one `loadlist`. Titles come from `#EXTINF`.
    pub async fn loadlist_append(&mut self, entries: &[(&str, &str)]) -> color_eyre::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.socket.with_file_name("append.m3u");
        self.loadlist(&path, playlist_m3u(entries.iter().copied()), None)
            .await
    }

    /// Splices every entry in at `index` in one `loadlist`. Playback is
    /// unaffected; mpv shifts `playlist-pos` by the number inserted.
    pub async fn loadlist_insert_at(
        &mut self,
        entries: &[(&str, &str)],
        index: usize,
    ) -> color_eyre::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.socket.with_file_name("insert.m3u");
        self.loadlist(&path, playlist_m3u(entries.iter().copied()), Some(index))
            .await
    }

    /// Writes an M3U next to the IPC socket, loads it, then removes it.
    /// A temp file is what gives each entry its title before it is opened.
    async fn loadlist(
        &mut self,
        path: &Path,
        body: String,
        index: Option<usize>,
    ) -> color_eyre::Result<()> {
        tokio::fs::write(path, body).await?;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await;
        let args: Vec<Value> = match index {
            Some(i) => loadlist_insert_at_args(&path.to_string_lossy(), i).to_vec(),
            None => loadlist_append_args(&path.to_string_lossy()).to_vec(),
        };
        let result = self.command(args).await;
        let _ = tokio::fs::remove_file(path).await;
        result?;
        Ok(())
    }

    pub async fn playlist_next(&mut self) -> color_eyre::Result<()> {
        self.command(vec![json!("playlist-next"), json!("force")])
            .await?;
        Ok(())
    }

    pub async fn playlist_prev(&mut self) -> color_eyre::Result<()> {
        self.command(vec![json!("playlist-prev"), json!("force")])
            .await?;
        Ok(())
    }

    pub async fn playlist_pos(&mut self) -> color_eyre::Result<i64> {
        Ok(self
            .get_property("playlist-pos")
            .await?
            .as_i64()
            .unwrap_or(0))
    }

    pub async fn playlist_count(&mut self) -> color_eyre::Result<i64> {
        Ok(self
            .get_property("playlist-count")
            .await?
            .as_i64()
            .unwrap_or(0))
    }

    pub async fn set_keep_open(&mut self) -> color_eyre::Result<()> {
        self.set_property("keep-open", json!(KEEP_OPEN)).await
    }

    pub async fn sub_add(&mut self, url: &str) -> color_eyre::Result<()> {
        self.command(vec![json!("sub-add"), json!(url)]).await?;
        Ok(())
    }

    pub async fn apply_auth_header(&mut self, header_field: &str) -> color_eyre::Result<()> {
        self.set_property("http-header-fields", json!([header_field]))
            .await
    }

    pub async fn clear_auth_header(&mut self) -> color_eyre::Result<()> {
        self.set_property("http-header-fields", json!([])).await
    }

    pub async fn pause(&mut self) -> color_eyre::Result<()> {
        self.set_property("pause", json!(true)).await
    }

    pub async fn unpause(&mut self) -> color_eyre::Result<()> {
        self.set_property("pause", json!(false)).await
    }

    pub async fn toggle_pause(&mut self) -> color_eyre::Result<()> {
        let paused = self.get_property("pause").await?.as_bool().unwrap_or(false);
        self.set_property("pause", json!(!paused)).await
    }

    pub async fn seek_absolute(&mut self, seconds: f64) -> color_eyre::Result<()> {
        self.command(vec![json!("seek"), json!(seconds), json!("absolute")])
            .await?;
        Ok(())
    }

    pub async fn set_volume(&mut self, volume: i64) -> color_eyre::Result<()> {
        self.set_property("volume", json!(volume.clamp(0, 100)))
            .await
    }

    pub async fn add_volume(&mut self, delta: i64) -> color_eyre::Result<i64> {
        let cur = self.get_property("volume").await?.as_f64().unwrap_or(100.0) as i64;
        let next = (cur + delta).clamp(0, 100);
        self.set_volume(next).await?;
        Ok(next)
    }

    pub async fn set_mute(&mut self, mute: bool) -> color_eyre::Result<()> {
        self.set_property("mute", json!(mute)).await
    }

    pub async fn toggle_mute(&mut self) -> color_eyre::Result<()> {
        let muted = self.get_property("mute").await?.as_bool().unwrap_or(false);
        self.set_mute(!muted).await
    }

    pub async fn set_aid(&mut self, aid: i64) -> color_eyre::Result<()> {
        self.set_property("aid", json!(aid)).await
    }

    pub async fn set_sid(&mut self, sid: Option<i64>) -> color_eyre::Result<()> {
        match sid {
            Some(s) if s >= 0 => self.set_property("sid", json!(s)).await,
            _ => self.set_property("sid", json!("no")).await,
        }
    }

    pub async fn max_sub_sid(&mut self) -> color_eyre::Result<i64> {
        let list = self.get_property("track-list").await?;
        Ok(max_sub_sid_from_track_list(&list))
    }

    pub async fn toggle_fullscreen(&mut self) -> color_eyre::Result<()> {
        let fs = self
            .get_property("fullscreen")
            .await?
            .as_bool()
            .unwrap_or(false);
        self.set_property("fullscreen", json!(!fs)).await
    }

    pub async fn time_pos(&mut self) -> color_eyre::Result<f64> {
        let v = self.get_property("time-pos").await?;
        json_as_seconds(&v).ok_or_else(|| eyre!("time-pos was not a number"))
    }

    pub async fn paused(&mut self) -> color_eyre::Result<bool> {
        Ok(self.get_property("pause").await?.as_bool().unwrap_or(false))
    }

    pub async fn volume(&mut self) -> color_eyre::Result<i64> {
        Ok(self.get_property("volume").await?.as_f64().unwrap_or(100.0) as i64)
    }

    pub async fn muted(&mut self) -> color_eyre::Result<bool> {
        Ok(self.get_property("mute").await?.as_bool().unwrap_or(false))
    }

    pub async fn quit_and_wait(&mut self) -> color_eyre::Result<()> {
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
                            Ok(IpcMessage::Event { name, reason, .. }) => {
                                let ev = match name.as_str() {
                                    "end-file" => Some(MpvEvent::EndFile {
                                        reason: reason.unwrap_or_else(|| "unknown".into()),
                                    }),
                                    "file-loaded" => Some(MpvEvent::FileLoaded),
                                    _ => None,
                                };
                                if let Some(ev) = ev
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
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    #[test]
    fn encode_is_one_json_line() {
        let line = encode_command(3, &[json!("get_property"), json!("pause")]);
        assert!(line.ends_with('\n'));
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["request_id"], 3);
        assert_eq!(v["command"][0], "get_property");
    }

    #[test]
    fn playlist_m3u_strips_newlines_from_title() {
        let body = playlist_m3u([("A\nB\rC", "http://h/x")]);
        assert_eq!(body, "#EXTM3U\n#EXTINF:-1,A B C\nhttp://h/x\n");
    }

    #[test]
    fn playlist_m3u_writes_every_entry() {
        let body = playlist_m3u([
            ("Show - s1e01 - One", "http://h/a"),
            ("Show - s1e02 - Two", "http://h/b"),
        ]);
        assert_eq!(
            body,
            "#EXTM3U\n#EXTINF:-1,Show - s1e01 - One\nhttp://h/a\n#EXTINF:-1,Show - s1e02 - Two\nhttp://h/b\n"
        );
    }

    #[test]
    fn loadlist_append_is_path_and_append_only() {
        let line = encode_command(1, &loadlist_append_args("/tmp/append.m3u"));
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let cmd = v["command"].as_array().unwrap();
        assert_eq!(cmd.len(), 3);
        assert_eq!(cmd[0], "loadlist");
        assert_eq!(cmd[1], "/tmp/append.m3u");
        assert_eq!(cmd[2], "append");
    }

    #[test]
    fn loadlist_insert_at_keeps_the_index_a_separate_argument() {
        // "insert-at0" as one token is `invalid parameter` in mpv.
        let line = encode_command(1, &loadlist_insert_at_args("/tmp/insert.m3u", 0));
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let cmd = v["command"].as_array().unwrap();
        assert_eq!(cmd.len(), 4);
        assert_eq!(cmd[0], "loadlist");
        assert_eq!(cmd[1], "/tmp/insert.m3u");
        assert_eq!(cmd[2], "insert-at");
        assert_eq!(cmd[3], 0);
    }

    #[test]
    fn loadlist_insert_at_uses_the_given_index() {
        let line = encode_command(1, &loadlist_insert_at_args("/tmp/insert.m3u", 3));
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let cmd = v["command"].as_array().unwrap();
        assert_eq!(cmd[3], 3);
    }

    #[test]
    fn time_pos_parses_integer_and_float_seconds() {
        assert_eq!(json_as_seconds(&json!(12)), Some(12.0));
        assert_eq!(json_as_seconds(&json!(12.5)), Some(12.5));
    }

    #[test]
    fn time_pos_rejects_a_non_number_instead_of_zero() {
        assert_eq!(json_as_seconds(&json!(null)), None);
        assert_eq!(json_as_seconds(&json!("unavailable")), None);
    }

    #[test]
    fn parse_reply_and_event() {
        let r = parse_ipc_line(r#"{"error":"success","data":12.5,"request_id":1}"#).unwrap();
        assert_eq!(
            r,
            IpcMessage::Reply {
                request_id: 1,
                error: "success".into(),
                data: json!(12.5),
            }
        );
        let e = parse_ipc_line(r#"{"event":"end-file","reason":"eof"}"#).unwrap();
        assert_eq!(
            e,
            IpcMessage::Event {
                name: "end-file".into(),
                reason: Some("eof".into()),
                data: json!({"event":"end-file","reason":"eof"}),
            }
        );
    }

    #[tokio::test]
    async fn ipc_roundtrip_against_fake_socket() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(v["command"][0], "get_property");
            let id = v["request_id"].as_i64().unwrap();
            let reply = format!(
                "{}\n",
                json!({"error":"success","data":true,"request_id": id})
            );
            reader.get_mut().write_all(reply.as_bytes()).await.unwrap();
            // keep the socket open until the client is done
            sleep(Duration::from_millis(200)).await;
        });

        let stream = UnixStream::connect(&sock).await.unwrap();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(ipc_loop(stream, cmd_rx, ev_tx));

        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(IpcCmd::Request {
                line: encode_command(1, &[json!("get_property"), json!("pause")]),
                id: 1,
                reply: tx,
            })
            .unwrap();
        let data = rx.await.unwrap().unwrap();
        assert_eq!(data, json!(true));
        let _ = cmd_tx.send(IpcCmd::Shutdown);
        let _ = server.await;
    }

    #[test]
    fn max_sub_sid_from_track_list_picks_the_highest_sub_id() {
        let list = json!([
            {"type": "audio", "id": 1},
            {"type": "sub", "id": 1},
            {"type": "sub", "id": 3},
            {"type": "video", "id": 1}
        ]);
        assert_eq!(max_sub_sid_from_track_list(&list), 3);
    }

    #[test]
    fn max_sub_sid_from_track_list_is_zero_when_empty() {
        assert_eq!(max_sub_sid_from_track_list(&json!([])), 0);
        assert_eq!(max_sub_sid_from_track_list(&json!(null)), 0);
    }
}
