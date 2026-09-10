//! The tracing subscriber jellytui installs and the ring buffer it writes to.
//!
//! Its only sink is memory: a `fmt` layer would write to stdout, which is the
//! alternate screen. See `specs/tui.md`.

use color_eyre::eyre::Result;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// Roughly an hour of `debug` browsing, and a few hundred KB at worst.
pub(crate) const CAPACITY: usize = 2000;

#[derive(Clone)]
pub(crate) struct LogLine {
    /// Since the subscriber was installed. A wall clock would mean a new
    /// dependency: `tracing-subscriber` is built without its `time` feature.
    pub(crate) elapsed: Duration,
    pub(crate) level: Level,
    pub(crate) target: String,
    pub(crate) message: String,
}

struct Ring {
    lines: VecDeque<LogLine>,
    /// Sequence number of `lines.front()`. Eviction moves every index, so the
    /// scroll anchor keys on this instead — the rule `specs/tui.md` already
    /// sets for search generations and level depths.
    first_seq: u64,
}

#[derive(Clone)]
pub(crate) struct LogBuffer(Arc<Mutex<Ring>>);

impl LogBuffer {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(Ring {
            lines: VecDeque::new(),
            first_seq: 0,
        })))
    }

    fn push(&self, line: LogLine) {
        let mut ring = self.lock();
        if ring.lines.len() == CAPACITY {
            ring.lines.pop_front();
            ring.first_seq += 1;
        }
        ring.lines.push_back(line);
    }

    /// The pane's tests need lines without a global subscriber to make them.
    #[cfg(test)]
    pub(crate) fn push_line(&self, message: String) {
        self.push(LogLine {
            elapsed: Duration::default(),
            level: Level::INFO,
            target: "test".into(),
            message,
        });
    }

    pub(crate) fn clear(&self) {
        let mut ring = self.lock();
        ring.first_seq += u64::try_from(ring.lines.len()).unwrap_or(u64::MAX);
        ring.lines.clear();
    }

    /// `(first_seq, len)`: everything the scroll arithmetic needs without
    /// copying the lines themselves.
    pub(crate) fn extent(&self) -> (u64, usize) {
        let ring = self.lock();
        (ring.first_seq, ring.lines.len())
    }

    pub(crate) fn window(&self, start: usize, len: usize) -> Vec<LogLine> {
        let ring = self.lock();
        ring.lines.iter().skip(start).take(len).cloned().collect()
    }

    /// A panic while a line was being pushed must not take the UI down too.
    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A `Layer` on the registry rather than a bespoke `Subscriber`: the registry
/// is what stores span data, so `#[instrument]` keeps working and another
/// layer can still be added. It costs `jellytui` 557 KB (+10.6%).
struct Capture {
    buffer: LogBuffer,
    started: Instant,
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        self.buffer.push(LogLine {
            elapsed: self.started.elapsed(),
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            message: message.0,
        });
    }
}

/// The `message` field first and bare, every other field after it as
/// `key=value` — a flat `fmt`-style line without the span context.
struct Message(String);

impl Message {
    fn write(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, "{}={value:?}", field.name());
        }
    }
}

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.write(field, value);
    }

    /// `Debug` on a `&str` would put quotes round every string field.
    fn record_str(&mut self, field: &Field, value: &str) {
        self.write(field, &format_args!("{value}"));
    }
}

pub(crate) fn install(level: &str) -> Result<LogBuffer> {
    let buffer = LogBuffer::new();
    tracing_subscriber::registry()
        .with(jellysink_core::logging::log_filter(level)?)
        .with(Capture {
            buffer: buffer.clone(),
            started: Instant::now(),
        })
        .init();
    Ok(buffer)
}

#[cfg(test)]
#[path = "logs_test.rs"]
mod tests;
