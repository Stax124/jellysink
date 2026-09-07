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
    /// The new value is dropped — see [`MpvEvent::SubtitleTrackChanged`].
    PropertyChange { property: String },
}

pub(crate) fn encode_command(request_id: i64, args: &[Value]) -> String {
    let v = json!({
        "command": args,
        "request_id": request_id,
    });
    format!("{v}\n")
}

/// M3U with one `#EXTINF` entry per `(title, url)`. The only way to give
/// unloaded playlist entries a title.
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

pub(crate) fn loadlist_append_args(path: &str) -> [Value; 3] {
    [json!("loadlist"), json!(path), json!("append")]
}

/// `insert-at` and the index must stay separate arguments; `"insert-at0"` is
/// `invalid parameter`.
pub(crate) fn loadlist_insert_at_args(path: &str, index: usize) -> [Value; 4] {
    [
        json!("loadlist"),
        json!(path),
        json!("insert-at"),
        json!(index),
    ]
}

/// `yes` auto-plays the rest of the playlist and emits the `end-file` autoplay
/// keys off; `always` unloads nothing and never emits it.
pub(crate) const KEEP_OPEN: &str = "yes";

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

/// Coerce an mpv property answer, or say what we actually got. No plausible
/// fallback value: callers make autoplay decisions from these numbers.
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

/// The mpv property holding the selected audio track.
pub(crate) const AUDIO_TRACK_PROPERTY: &str = "aid";

/// `observe_property` id for [`SUBTITLE_TRACK_PROPERTY`]. We match on the
/// property name, so only being distinct from other observers matters.
const SUBTITLE_TRACK_OBSERVER_ID: i64 = 1;

/// See [`SUBTITLE_TRACK_OBSERVER_ID`]; only has to differ from it.
const AUDIO_TRACK_OBSERVER_ID: i64 = 2;

/// What mpv answers for a track-id property such as `sid`. `false` (off) and
/// `auto` (not picked yet) must stay apart: a loading file is not a decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SelectedTrack {
    /// This track is selected.
    Id(i64),
    /// Explicitly off.
    Off,
    /// `auto`: mpv has not picked a track yet. Never a decision, and so the
    /// state every file starts and ends in.
    #[default]
    Unresolved,
}

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

/// Why mpv ended a file. An enum rather than a `String` so `end_file_action`
/// can match exhaustively.
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

#[derive(Debug, Clone)]
pub(crate) enum MpvEvent {
    EndFile {
        reason: EndFileReason,
    },
    FileLoaded,
    /// mpv's selected subtitle track changed — `j` in the mpv window, its track
    /// menu, or mpv auto-selecting one as a file loads.
    ///
    /// Carries no track id on purpose: these are handled a whole file load
    /// after they are emitted, so the runtime re-reads `sid` instead.
    SubtitleTrackChanged,
    /// mpv's selected audio track changed — `#` in the mpv window, its track
    /// menu, or mpv auto-selecting one as a file loads.
    ///
    /// Carries no track id, for the same reason
    /// [`MpvEvent::SubtitleTrackChanged`] does not.
    AudioTrackChanged,
    Exited,
}

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

    pub(crate) async fn loadfile(
        &mut self,
        url: &str,
        title: Option<&str>,
    ) -> color_eyre::Result<()> {
        // Since mpv 0.38 loadfile's 4th argument is an insert index, not an
        // options map, so force-media-title has to go through a property.
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
    /// unaffected; mpv shifts `playlist-pos`.
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

    /// Writes an M3U next to the IPC socket, loads it, then removes it. The
    /// file is what carries each entry's title.
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

    /// `None` or a negative id means `aid=no`, where `cycle audio` lands after
    /// the last track.
    pub(crate) async fn set_audio_track_id(
        &mut self,
        audio_track_id: Option<i64>,
    ) -> color_eyre::Result<()> {
        match audio_track_id {
            Some(id) if id >= 0 => self.set_property(AUDIO_TRACK_PROPERTY, json!(id)).await,
            _ => self.set_property(AUDIO_TRACK_PROPERTY, json!("no")).await,
        }
    }

    pub(crate) async fn audio_track(&mut self) -> color_eyre::Result<SelectedTrack> {
        Ok(selected_track_from_property(
            &self.get_property(AUDIO_TRACK_PROPERTY).await?,
        ))
    }

    /// So a track picked in the mpv window, not a Jellyfin client, is noticed.
    pub(crate) async fn observe_audio_track(&mut self) -> color_eyre::Result<()> {
        self.command(vec![
            json!("observe_property"),
            json!(AUDIO_TRACK_OBSERVER_ID),
            json!(AUDIO_TRACK_PROPERTY),
        ])
        .await?;
        Ok(())
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

    pub(crate) async fn subtitle_track(&mut self) -> color_eyre::Result<SelectedTrack> {
        Ok(selected_track_from_property(
            &self.get_property(SUBTITLE_TRACK_PROPERTY).await?,
        ))
    }

    /// So a track picked in the mpv window, not a Jellyfin client, is noticed.
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

/// Creates the file 0600 rather than chmodding after: the M3U body can carry
/// `ApiKey=`, so it must never exist world-readable, not even briefly.
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
    // tokio's File does not flush on drop, and mpv reads the path back at once.
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
            AUDIO_TRACK_PROPERTY => Some(MpvEvent::AudioTrackChanged),
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
#[path = "mpv_test.rs"]
mod tests;

#[cfg(test)]
#[path = "mpv_integration_test.rs"]
mod integration_tests;
