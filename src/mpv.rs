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

/// The mpv property holding the selected audio track.
pub(crate) const AUDIO_TRACK_PROPERTY: &str = "aid";

/// The `observe_property` id for [`SUBTITLE_TRACK_PROPERTY`].
///
/// mpv wants an id per observer and echoes it back on every change; we match on
/// the property name instead, so the only thing that matters is that ids of
/// different observers differ.
const SUBTITLE_TRACK_OBSERVER_ID: i64 = 1;

/// The `observe_property` id for [`AUDIO_TRACK_PROPERTY`]. See above: it only
/// has to differ from [`SUBTITLE_TRACK_OBSERVER_ID`].
const AUDIO_TRACK_OBSERVER_ID: i64 = 2;

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

    /// `None` (or a negative id) is an explicit `aid=no`, which is where mpv's
    /// `cycle audio` lands after the last track.
    pub(crate) async fn set_audio_track_id(
        &mut self,
        audio_track_id: Option<i64>,
    ) -> color_eyre::Result<()> {
        match audio_track_id {
            Some(id) if id >= 0 => self.set_property(AUDIO_TRACK_PROPERTY, json!(id)).await,
            _ => self.set_property(AUDIO_TRACK_PROPERTY, json!("no")).await,
        }
    }

    /// The selected audio track, as mpv currently has it.
    pub(crate) async fn audio_track(&mut self) -> color_eyre::Result<SelectedTrack> {
        Ok(selected_track_from_property(
            &self.get_property(AUDIO_TRACK_PROPERTY).await?,
        ))
    }

    /// Asks mpv to report every audio track change, so a track picked in the
    /// mpv window — not in a Jellyfin client — is noticed too.
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
