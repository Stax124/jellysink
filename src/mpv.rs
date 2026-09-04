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
pub(crate) enum IpcMessage {
    Reply {
        request_id: i64,
        error: String,
        data: Value,
    },
    Event {
        name: String,
        reason: Option<String>,
    },
    /// An `observe_property` notification. The new value is deliberately
    /// dropped — see [`MpvEvent::SubtitleTrackChanged`].
    PropertyChange { property: String },
}

/// Encodes a command to be sent to mpv
pub(crate) fn encode_command(request_id: i64, args: &[Value]) -> String {
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
pub(crate) fn playlist_m3u<I, T, U>(entries: I) -> String
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
pub(crate) fn loadlist_append_args(path: &str) -> [Value; 3] {
    [json!("loadlist"), json!(path), json!("append")]
}

/// Args for `loadlist` `insert-at` to splice entries in at `index`.
///
/// `insert-at` and the index are separate arguments; `"insert-at0"` as a single
/// token is `invalid parameter`. Inserting at or below the current position
/// does not interrupt playback — mpv shifts `playlist-pos` by the number
/// inserted and keeps playing the same file.
pub(crate) fn loadlist_insert_at_args(path: &str, index: usize) -> [Value; 4] {
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
pub(crate) const KEEP_OPEN: &str = "yes";

/// Parses an IPC line from mpv into an [`IpcMessage`]
pub(crate) fn parse_ipc_line(line: &str) -> color_eyre::Result<IpcMessage> {
    let v: Value = serde_json::from_str(line.trim()).wrap_err("mpv IPC JSON")?;
    if let Some(name) = v.get("event").and_then(Value::as_str) {
        if name == "property-change" {
            return Ok(IpcMessage::PropertyChange {
                property: v
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        let reason = v.get("reason").and_then(Value::as_str).map(str::to_string);
        return Ok(IpcMessage::Event {
            name: name.to_string(),
            reason,
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
/// Coerce an mpv property answer, or say what we actually got.
///
/// These used to fall back to a plausible value (`playlist-pos` → 0, `volume`
/// → 100), so callers made autoplay and reporting decisions from a number mpv
/// never gave us and a transient IPC hiccup played the wrong episode. An
/// mpv-level failure already comes back as `Err` from `command`; this covers a
/// success carrying the wrong JSON type.
fn as_i64_property(name: &str, v: &Value) -> color_eyre::Result<i64> {
    v.as_i64()
        .ok_or_else(|| eyre!("mpv property {name:?} was not an integer: {v}"))
}

fn as_f64_property(name: &str, v: &Value) -> color_eyre::Result<f64> {
    v.as_f64()
        .ok_or_else(|| eyre!("mpv property {name:?} was not a number: {v}"))
}

fn as_bool_property(name: &str, v: &Value) -> color_eyre::Result<bool> {
    v.as_bool()
        .ok_or_else(|| eyre!("mpv property {name:?} was not a boolean: {v}"))
}

pub(crate) fn json_as_seconds(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_u64().map(|n| n as f64))
}

/// Highest mpv subtitle track id (`sid`) in a track-list, typically after `sub-add`.
pub(crate) fn max_subtitle_track_id_from_track_list(list: &Value) -> i64 {
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

/// The mpv property holding the selected subtitle track.
pub(crate) const SUBTITLE_TRACK_PROPERTY: &str = "sid";

/// The `observe_property` id for [`SUBTITLE_TRACK_PROPERTY`].
///
/// mpv wants an id per observer and echoes it back on every change; we match on
/// the property name instead, so the only thing that matters is that ids of
/// different observers differ.
const SUBTITLE_TRACK_OBSERVER_ID: i64 = 1;

/// What mpv answers for a track-id property such as `sid`.
///
/// It is not just a number: mpv reports an explicit `no` as `false` and a
/// selection it has not made yet as `auto`. Collapsing those two into "no
/// track" would read a file that is still loading as the user switching
/// subtitles off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectedTrack {
    /// This track is selected.
    Id(i64),
    /// Explicitly off.
    Off,
    /// `auto`: mpv has not picked a track yet. Never a decision.
    Unresolved,
}

/// Reads a track-id property answer.
pub(crate) fn selected_track_from_property(v: &Value) -> SelectedTrack {
    match v {
        Value::Bool(false) => SelectedTrack::Off,
        Value::String(s) if s == "no" => SelectedTrack::Off,
        // A number mpv cannot fit in an i64 is not a track id we could use.
        Value::Number(n) => n
            .as_i64()
            .map_or(SelectedTrack::Unresolved, SelectedTrack::Id),
        _ => SelectedTrack::Unresolved,
    }
}

/// Why mpv ended a file.
///
/// Parsed once here rather than carried up as a `String` and string-matched in
/// three separate places, so `end_file_action` can match exhaustively and a
/// typo cannot silently fall into the ignore arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndFileReason {
    /// Played through to the end.
    Eof,
    /// mpv followed the file to another URL.
    Redirect,
    /// Playback was stopped — by the user, or by `playlist-next` / an OSC jump
    /// moving off the current entry.
    Stop,
    /// mpv is exiting.
    Quit,
    Error,
    /// A reason mpv added later, or no `reason` field at all.
    Other,
}

impl EndFileReason {
    fn parse(reason: Option<&str>) -> Self {
        match reason {
            Some("eof") => Self::Eof,
            Some("redirect") => Self::Redirect,
            Some("stop") => Self::Stop,
            Some("quit") => Self::Quit,
            Some("error") => Self::Error,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for EndFileReason {
    /// mpv's own spelling, so log lines read the same as mpv's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Eof => "eof",
            Self::Redirect => "redirect",
            Self::Stop => "stop",
            Self::Quit => "quit",
            Self::Error => "error",
            Self::Other => "unknown",
        })
    }
}

/// Represents an event received from mpv
#[derive(Debug, Clone)]
pub(crate) enum MpvEvent {
    EndFile {
        reason: EndFileReason,
    },
    FileLoaded,
    /// mpv's selected subtitle track changed — `j` in the mpv window, its track
    /// menu, or mpv auto-selecting one as a file loads.
    ///
    /// Carries no track id on purpose. Property changes arrive on their own
    /// channel and are handled a whole file load later than they were emitted,
    /// so the value in the message is routinely stale: mpv's auto-selection
    /// reaches the runtime *after* we have applied our own choice over it. The
    /// runtime re-reads `sid` and compares it with the selection it last
    /// settled on, which turns every stale event into a no-op.
    SubtitleTrackChanged,
    Exited,
}

struct Pending {
    tx: oneshot::Sender<Result<Value, String>>,
}

/// Drops pending requests whose caller has gone away — timed out (`command`
/// gives up after 10 s) or had its future cancelled by a `select!`.
///
/// mpv never replies to a command it did not process, so those entries were
/// never removed: `pending` grew for the life of the session, leaking a
/// `oneshot::Sender` per abandoned request.
fn evict_abandoned(pending: &mut HashMap<i64, Pending>) -> usize {
    let before = pending.len();
    pending.retain(|_, p| !p.tx.is_closed());
    before - pending.len()
}

/// Represents a session with an mpv process
pub(crate) struct MpvSession {
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
        // Belt and braces on top of the 0700 config directory: mpv creates this
        // socket under the ambient umask, and `http-header-fields` on it carries
        // the Jellyfin access token.
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

    /// Sends a command to the mpv process and returns the response
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

    pub(crate) async fn loadfile(
        &mut self,
        url: &str,
        title: Option<&str>,
    ) -> color_eyre::Result<()> {
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
    pub(crate) async fn loadlist_append(
        &mut self,
        entries: &[(&str, &str)],
    ) -> color_eyre::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.socket.with_file_name("append.m3u");
        self.loadlist(&path, playlist_m3u(entries.iter().copied()), None)
            .await
    }

    /// Splices every entry in at `index` in one `loadlist`. Playback is
    /// unaffected; mpv shifts `playlist-pos` by the number inserted.
    pub(crate) async fn loadlist_insert_at(
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
        write_private(path, &body).await?;
        let args: Vec<Value> = match index {
            Some(i) => loadlist_insert_at_args(&path.to_string_lossy(), i).to_vec(),
            None => loadlist_append_args(&path.to_string_lossy()).to_vec(),
        };
        let result = self.command(args).await;
        let _ = tokio::fs::remove_file(path).await;
        result?;
        Ok(())
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

    pub(crate) async fn playlist_next(&mut self) -> color_eyre::Result<()> {
        self.command(vec![json!("playlist-next"), json!("force")])
            .await?;
        Ok(())
    }

    pub(crate) async fn playlist_prev(&mut self) -> color_eyre::Result<()> {
        self.command(vec![json!("playlist-prev"), json!("force")])
            .await?;
        Ok(())
    }

    /// mpv reports `-1` while idle; that is a real answer, not a failure.
    pub(crate) async fn playlist_pos(&mut self) -> color_eyre::Result<i64> {
        self.get_i64("playlist-pos").await
    }

    pub(crate) async fn playlist_count(&mut self) -> color_eyre::Result<i64> {
        self.get_i64("playlist-count").await
    }

    pub(crate) async fn set_keep_open(&mut self) -> color_eyre::Result<()> {
        self.set_property("keep-open", json!(KEEP_OPEN)).await
    }

    pub(crate) async fn sub_add(&mut self, url: &str) -> color_eyre::Result<()> {
        self.command(vec![json!("sub-add"), json!(url)]).await?;
        Ok(())
    }

    pub(crate) async fn apply_auth_header(&mut self, header_field: &str) -> color_eyre::Result<()> {
        self.set_property("http-header-fields", json!([header_field]))
            .await
    }

    pub(crate) async fn clear_auth_header(&mut self) -> color_eyre::Result<()> {
        self.set_property("http-header-fields", json!([])).await
    }

    pub(crate) async fn pause(&mut self) -> color_eyre::Result<()> {
        self.set_property("pause", json!(true)).await
    }

    pub(crate) async fn unpause(&mut self) -> color_eyre::Result<()> {
        self.set_property("pause", json!(false)).await
    }

    pub(crate) async fn toggle_pause(&mut self) -> color_eyre::Result<()> {
        let paused = self.get_bool("pause").await?;
        self.set_property("pause", json!(!paused)).await
    }

    pub(crate) async fn seek_absolute(&mut self, seconds: f64) -> color_eyre::Result<()> {
        self.command(vec![json!("seek"), json!(seconds), json!("absolute")])
            .await?;
        Ok(())
    }

    pub(crate) async fn set_volume(&mut self, volume: i64) -> color_eyre::Result<()> {
        self.set_property("volume", json!(volume.clamp(0, 100)))
            .await
    }

    pub(crate) async fn add_volume(&mut self, delta: i64) -> color_eyre::Result<i64> {
        let cur = self.get_f64("volume").await? as i64;
        let next = (cur + delta).clamp(0, 100);
        self.set_volume(next).await?;
        Ok(next)
    }

    pub(crate) async fn set_mute(&mut self, mute: bool) -> color_eyre::Result<()> {
        self.set_property("mute", json!(mute)).await
    }

    pub(crate) async fn set_audio_track_id(
        &mut self,
        audio_track_id: i64,
    ) -> color_eyre::Result<()> {
        self.set_property("aid", json!(audio_track_id)).await
    }

    pub(crate) async fn set_subtitle_track_id(
        &mut self,
        subtitle_track_id: Option<i64>,
    ) -> color_eyre::Result<()> {
        match subtitle_track_id {
            Some(id) if id >= 0 => self.set_property("sid", json!(id)).await,
            _ => self.set_property("sid", json!("no")).await,
        }
    }

    /// The selected subtitle track, as mpv currently has it.
    pub(crate) async fn subtitle_track(&mut self) -> color_eyre::Result<SelectedTrack> {
        Ok(selected_track_from_property(
            &self.get_property(SUBTITLE_TRACK_PROPERTY).await?,
        ))
    }

    /// Asks mpv to report every subtitle track change, so a track picked in the
    /// mpv window — not in a Jellyfin client — is noticed too.
    pub(crate) async fn observe_subtitle_track(&mut self) -> color_eyre::Result<()> {
        self.command(vec![
            json!("observe_property"),
            json!(SUBTITLE_TRACK_OBSERVER_ID),
            json!(SUBTITLE_TRACK_PROPERTY),
        ])
        .await?;
        Ok(())
    }

    pub(crate) async fn max_subtitle_track_id(&mut self) -> color_eyre::Result<i64> {
        let list = self.get_property("track-list").await?;
        Ok(max_subtitle_track_id_from_track_list(&list))
    }

    pub(crate) async fn toggle_fullscreen(&mut self) -> color_eyre::Result<()> {
        let fs = self.get_bool("fullscreen").await?;
        self.set_property("fullscreen", json!(!fs)).await
    }

    pub(crate) async fn time_pos(&mut self) -> color_eyre::Result<f64> {
        let v = self.get_property("time-pos").await?;
        json_as_seconds(&v).ok_or_else(|| eyre!("time-pos was not a number"))
    }

    pub(crate) async fn paused(&mut self) -> color_eyre::Result<bool> {
        self.get_bool("pause").await
    }

    pub(crate) async fn volume(&mut self) -> color_eyre::Result<i64> {
        Ok(self.get_f64("volume").await? as i64)
    }

    pub(crate) async fn muted(&mut self) -> color_eyre::Result<bool> {
        self.get_bool("mute").await
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

/// Writes a file only the current user can read, creating it with the mode
/// rather than chmodding after.
///
/// The M3U body carries `ApiKey=` whenever the Authorization header is not in
/// play. `fs::write` + `set_permissions` left it at `0644 & ~umask` in between,
/// so the token was briefly world-readable. Unlinking first means a stale file
/// left by a crash cannot donate its old, looser mode.
async fn write_private(path: &Path, body: &str) -> color_eyre::Result<()> {
    let _ = tokio::fs::remove_file(path).await;
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await
        .wrap_err_with(|| format!("creating {}", path.display()))?;
    f.write_all(body.as_bytes())
        .await
        .wrap_err_with(|| format!("writing {}", path.display()))?;
    // tokio's File does not flush on drop, and mpv reads this path back
    // immediately — without this it loads an empty playlist.
    f.flush()
        .await
        .wrap_err_with(|| format!("flushing {}", path.display()))?;
    Ok(())
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

/// The runtime-level event an inbound mpv message means, if any.
fn mpv_event_for(msg: &IpcMessage) -> Option<MpvEvent> {
    match msg {
        IpcMessage::Event { name, reason } => match name.as_str() {
            "end-file" => Some(MpvEvent::EndFile {
                reason: EndFileReason::parse(reason.as_deref()),
            }),
            "file-loaded" => Some(MpvEvent::FileLoaded),
            _ => None,
        },
        IpcMessage::PropertyChange { property } => match property.as_str() {
            SUBTITLE_TRACK_PROPERTY => Some(MpvEvent::SubtitleTrackChanged),
            _ => None,
        },
        IpcMessage::Reply { .. } => None,
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
    fn a_property_change_parses_to_the_property_name() {
        assert_eq!(
            parse_ipc_line(r#"{"event":"property-change","id":1,"name":"sid","data":3}"#).unwrap(),
            IpcMessage::PropertyChange {
                property: "sid".into()
            }
        );
    }

    #[test]
    fn only_the_observed_subtitle_property_becomes_an_event() {
        assert!(matches!(
            mpv_event_for(&IpcMessage::PropertyChange {
                property: SUBTITLE_TRACK_PROPERTY.into()
            }),
            Some(MpvEvent::SubtitleTrackChanged)
        ));
        assert!(
            mpv_event_for(&IpcMessage::PropertyChange {
                property: "aid".into()
            })
            .is_none()
        );
    }

    /// The whole point of [`SelectedTrack`]: `no` and `auto` are different
    /// answers, and reading `auto` as "off" would record a file that is still
    /// loading as the user switching subtitles off.
    #[test]
    fn a_track_property_tells_off_apart_from_not_yet_decided() {
        assert_eq!(
            selected_track_from_property(&json!(3)),
            SelectedTrack::Id(3)
        );
        assert_eq!(
            selected_track_from_property(&json!(false)),
            SelectedTrack::Off
        );
        assert_eq!(
            selected_track_from_property(&json!("no")),
            SelectedTrack::Off
        );
        assert_eq!(
            selected_track_from_property(&json!("auto")),
            SelectedTrack::Unresolved
        );
        assert_eq!(
            selected_track_from_property(&Value::Null),
            SelectedTrack::Unresolved
        );
    }

    #[test]
    fn observe_property_sends_an_id_and_the_property_name() {
        let line = encode_command(
            7,
            &[
                json!("observe_property"),
                json!(SUBTITLE_TRACK_OBSERVER_ID),
                json!(SUBTITLE_TRACK_PROPERTY),
            ],
        );
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let cmd = v["command"].as_array().unwrap();
        assert_eq!(cmd[0], "observe_property");
        assert_eq!(cmd[1], SUBTITLE_TRACK_OBSERVER_ID);
        assert_eq!(cmd[2], "sid");
    }

    #[test]
    fn max_subtitle_track_id_from_track_list_picks_the_highest_sub_id() {
        let list = json!([
            {"type": "audio", "id": 1},
            {"type": "sub", "id": 1},
            {"type": "sub", "id": 3},
            {"type": "video", "id": 1}
        ]);
        assert_eq!(max_subtitle_track_id_from_track_list(&list), 3);
    }

    #[test]
    fn max_subtitle_track_id_from_track_list_is_zero_when_empty() {
        assert_eq!(max_subtitle_track_id_from_track_list(&json!([])), 0);
        assert_eq!(max_subtitle_track_id_from_track_list(&json!(null)), 0);
    }

    #[test]
    fn property_coercions_accept_the_expected_shapes() {
        assert_eq!(as_i64_property("playlist-pos", &json!(3)).unwrap(), 3);
        // mpv reports -1 for playlist-pos while idle; that is a real answer.
        assert_eq!(as_i64_property("playlist-pos", &json!(-1)).unwrap(), -1);
        assert_eq!(as_f64_property("volume", &json!(62.5)).unwrap(), 62.5);
        assert!(as_bool_property("pause", &json!(true)).unwrap());
    }

    /// The regression: a null or wrong-typed answer used to become 0 / 100.0 /
    /// false, and `playlist_eof` then picked an episode from it.
    #[test]
    fn property_coercions_reject_a_missing_or_wrong_typed_answer() {
        for v in [json!(null), json!("3"), json!({})] {
            assert!(
                as_i64_property("playlist-pos", &v).is_err(),
                "{v} should not coerce to an integer"
            );
        }
        assert!(as_f64_property("volume", &json!(null)).is_err());
        assert!(as_bool_property("pause", &json!(null)).is_err());
        assert!(as_bool_property("pause", &json!(1)).is_err());
    }

    #[test]
    fn a_coercion_error_names_the_property_and_what_arrived() {
        let err = as_i64_property("playlist-count", &json!("nope")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("playlist-count"), "{msg}");
        assert!(msg.contains("nope"), "{msg}");
    }

    fn pending_entry() -> (Pending, oneshot::Receiver<Result<Value, String>>) {
        let (tx, rx) = oneshot::channel();
        (Pending { tx }, rx)
    }

    #[test]
    fn abandoned_requests_are_evicted() {
        let mut pending = HashMap::new();
        let (live, _live_rx) = pending_entry();
        let (abandoned, abandoned_rx) = pending_entry();
        pending.insert(1, live);
        pending.insert(2, abandoned);

        // The caller timed out and dropped its receiver.
        drop(abandoned_rx);

        assert_eq!(evict_abandoned(&mut pending), 1);
        assert!(pending.contains_key(&1), "a live waiter must be kept");
        assert!(!pending.contains_key(&2));
    }

    #[test]
    fn evicting_leaves_a_map_of_live_waiters_alone() {
        let mut pending = HashMap::new();
        let (a, _a_rx) = pending_entry();
        let (b, _b_rx) = pending_entry();
        pending.insert(1, a);
        pending.insert(2, b);
        assert_eq!(evict_abandoned(&mut pending), 0);
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn playlist_files_are_created_private_not_chmodded_after() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("append.m3u");
        write_private(&path, "#EXTM3U\n").await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the body can carry ApiKey=");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "#EXTM3U\n");
    }

    /// A file left behind by a crash must not donate its looser mode.
    #[tokio::test]
    async fn a_stale_world_readable_playlist_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("insert.m3u");
        std::fs::write(&path, "stale").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&path, "fresh").await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh");
    }

    #[test]
    fn end_file_reasons_parse_to_their_variants() {
        assert_eq!(EndFileReason::parse(Some("eof")), EndFileReason::Eof);
        assert_eq!(
            EndFileReason::parse(Some("redirect")),
            EndFileReason::Redirect
        );
        assert_eq!(EndFileReason::parse(Some("stop")), EndFileReason::Stop);
        assert_eq!(EndFileReason::parse(Some("quit")), EndFileReason::Quit);
        assert_eq!(EndFileReason::parse(Some("error")), EndFileReason::Error);
    }

    #[test]
    fn an_unknown_or_missing_reason_becomes_other() {
        assert_eq!(EndFileReason::parse(None), EndFileReason::Other);
        assert_eq!(
            EndFileReason::parse(Some("something-new")),
            EndFileReason::Other
        );
    }

    /// Log lines should keep reading like mpv's own.
    #[test]
    fn display_round_trips_mpv_spelling() {
        for name in ["eof", "redirect", "stop", "quit", "error"] {
            assert_eq!(EndFileReason::parse(Some(name)).to_string(), name);
        }
        assert_eq!(EndFileReason::Other.to_string(), "unknown");
    }
}
