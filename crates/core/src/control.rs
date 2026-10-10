//! The `control.sock` protocol: newline-delimited JSON between the daemon and
//! its local clients. See `specs/session.md`.

use crate::cast::CastEvent;
use crate::config::Paths;
use crate::status::PlayerStatus;
use crate::{APP_NAME, VERSION, usage_err};
use color_eyre::eyre::eyre;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// Bumped only for a breaking change; additions are new fields or new types.
pub const PROTOCOL: u32 = 1;
pub(crate) const MAX_LINE: usize = 64 * 1024;

/// How long a running daemon may take to welcome a client or answer it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(1);

/// An enum of one so its `type` is checked: a struct's tag is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Handshake {
    Hello { protocol: u32, client: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Restart,
    /// Carries its own `command` tag, so every controller speaks one vocabulary.
    #[serde(untagged)]
    Cast(CastEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonFrame {
    Welcome {
        protocol: u32,
        daemon: String,
        client_id: u64,
    },
    Status {
        status: Box<PlayerStatus>,
    },
    /// The command reached the daemon; what it did arrives as events.
    Reply {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Event(Event),
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    PlayLoading {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        origin: Origin,
    },
    PlayStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        origin: Origin,
    },
    PlayFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        origin: Origin,
        error: String,
    },
    PlayCancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        origin: Origin,
        by: Origin,
    },
    Reported {
        report: ReportKind,
        item_id: String,
        ok: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Jellyfin,
    Mpris,
    /// Autoplay and mpv's own playlist moves.
    Daemon,
    #[serde(untagged)]
    Client {
        client: u64,
        request: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    Start,
    Stopped,
}

#[derive(Debug)]
pub struct FrameReader<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
            line: Vec::new(),
        }
    }

    /// Cancel-safe: a partial line stays buffered for the next call. `None` at
    /// the end of the stream.
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let budget = (MAX_LINE + 1).saturating_sub(self.line.len()) as u64;
            let read = (&mut self.reader)
                .take(budget)
                .read_until(b'\n', &mut self.line)
                .await?;
            if self.line.last() == Some(&b'\n') {
                self.line.pop();
                let line = String::from_utf8(std::mem::take(&mut self.line))
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                return Ok(Some(line));
            }
            if self.line.len() > MAX_LINE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "a control line over 64 KiB",
                ));
            }
            if read == 0 {
                return Ok(None);
            }
        }
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &impl Serialize,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    writer.write_all(&line).await
}

/// Refused, gone, or accepted and then dropped unanswered: how `control.sock`
/// fails both as a SIGKILL leftover and while the daemon is between binds.
fn means_not_running(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

#[derive(Debug)]
pub struct Client {
    frames: FrameReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    sock: PathBuf,
    id: u64,
}

impl Client {
    /// The status is the snapshot that follows the welcome.
    pub async fn connect(paths: &Paths, name: &str) -> color_eyre::Result<(Self, PlayerStatus)> {
        let sock = paths.control_socket();
        let stream = match UnixStream::connect(&sock).await {
            Ok(stream) => stream,
            Err(e) if means_not_running(&e) => return Err(usage_err("jellysink is not running")),
            Err(e) => return Err(eyre!(e).wrap_err(format!("connecting to {}", sock.display()))),
        };
        let (read, writer) = stream.into_split();
        let mut client = Self {
            frames: FrameReader::new(read),
            writer,
            sock,
            id: 0,
        };
        let hello = Handshake::Hello {
            protocol: PROTOCOL,
            client: name.to_string(),
        };
        match tokio::time::timeout(REPLY_TIMEOUT, client.greet(&hello)).await {
            Ok(status) => Ok((client, status?)),
            Err(_) => Err(usage_err("jellysink is not responding")),
        }
    }

    async fn greet(&mut self, hello: &Handshake) -> color_eyre::Result<PlayerStatus> {
        self.write(hello).await?;
        match self.next_frame().await? {
            Some(DaemonFrame::Welcome { client_id, .. }) => self.id = client_id,
            Some(DaemonFrame::Error { message }) => {
                return Err(usage_err(format!(
                    "jellysink refused the connection: {message}"
                )));
            }
            Some(frame) => {
                return Err(eyre!(
                    "{} answered a hello with {frame:?}",
                    self.sock.display()
                ));
            }
            None => return Err(usage_err("jellysink is not running")),
        }
        match self.next_frame().await? {
            Some(DaemonFrame::Status { status }) => Ok(*status),
            Some(frame) => Err(eyre!(
                "{} sent {frame:?} in place of its status",
                self.sock.display()
            )),
            None => Err(usage_err("jellysink is not running")),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub async fn send(&mut self, request: &Request) -> color_eyre::Result<()> {
        self.write(request).await
    }

    async fn write(&mut self, frame: &impl Serialize) -> color_eyre::Result<()> {
        write_frame(&mut self.writer, frame)
            .await
            .map_err(|e| self.io_error(e))
    }

    /// Cancel-safe. `None` once the daemon closes; a frame of a type this
    /// client does not know is skipped.
    pub async fn next_frame(&mut self) -> color_eyre::Result<Option<DaemonFrame>> {
        loop {
            let line = match self.frames.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => return Ok(None),
                Err(e) => return Err(self.io_error(e)),
            };
            match serde_json::from_str(&line) {
                Ok(frame) => return Ok(Some(frame)),
                Err(e) => {
                    tracing::debug!("skipping a control frame this client does not know: {e}")
                }
            }
        }
    }

    fn io_error(&self, e: std::io::Error) -> color_eyre::Report {
        if means_not_running(&e) {
            usage_err("jellysink is not running")
        } else {
            eyre!(e).wrap_err(format!("talking to {}", self.sock.display()))
        }
    }
}

/// Replied to before the daemon fires it, so the updater knows it landed.
pub(crate) async fn request_restart(paths: &Paths) -> color_eyre::Result<()> {
    let (mut client, _) = Client::connect(paths, &format!("{APP_NAME} {VERSION} updater")).await?;
    let restart = Request {
        id: 1,
        command: Command::Restart,
    };
    client.send(&restart).await?;
    let reply = tokio::time::timeout(REPLY_TIMEOUT, async {
        loop {
            match client.next_frame().await? {
                Some(DaemonFrame::Reply { id: 1, error }) => return Ok(error),
                Some(_) => {}
                None => {
                    return Err(usage_err(
                        "jellysink closed the connection before answering",
                    ));
                }
            }
        }
    })
    .await
    .map_err(|_| usage_err("jellysink is not responding"))??;
    match reply {
        None => Ok(()),
        Some(error) => Err(eyre!("jellysink refused to restart: {error}")),
    }
}

#[cfg(test)]
#[path = "control_test.rs"]
mod tests;
