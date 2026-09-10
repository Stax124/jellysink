use super::*;

/// Scoped to this call rather than `install`'s global subscriber, so each test
/// has a buffer of its own.
fn capture(f: impl FnOnce()) -> LogBuffer {
    let buffer = LogBuffer::new();
    let subscriber = tracing_subscriber::registry().with(Capture {
        buffer: buffer.clone(),
        started: Instant::now(),
    });
    tracing::subscriber::with_default(subscriber, f);
    buffer
}

fn lines(buffer: &LogBuffer) -> Vec<LogLine> {
    let (_, len) = buffer.extent();
    buffer.window(0, len)
}

#[test]
fn records_level_target_and_fields() {
    let buffer = capture(|| {
        tracing::info!(item = "The Bear", elapsed_ms = 12, "played");
    });

    let lines = lines(&buffer);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].level, Level::INFO);
    assert_eq!(lines[0].target, "jellytui::logs::tests");
    assert_eq!(lines[0].message, "played item=The Bear elapsed_ms=12");
}

#[test]
fn oldest_lines_are_evicted_and_the_sequence_keeps_counting() {
    let buffer = capture(|| {
        for n in 0..CAPACITY + 5 {
            tracing::info!("line {n}");
        }
    });

    let (first_seq, len) = buffer.extent();
    assert_eq!(len, CAPACITY);
    assert_eq!(first_seq, 5);
    assert_eq!(buffer.window(0, 1)[0].message, "line 5");
}

#[test]
fn clearing_advances_the_sequence_past_what_was_dropped() {
    let buffer = capture(|| tracing::info!("before"));
    buffer.clear();

    assert_eq!(buffer.extent(), (1, 0));
}
