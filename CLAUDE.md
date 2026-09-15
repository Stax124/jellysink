# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project

jellysink is a headless Jellyfin cast target for Linux, written in Rust (edition 2024). It registers as a remote player, receives Play commands over a WebSocket, and DirectPlays the stream in the user's installed `mpv`.

## Commands

```bash
cargo build                                             # both binaries
cargo test                                              # the whole workspace
cargo fmt                                               # format; `cargo fmt --check` must stay clean
cargo clippy --workspace --all-targets -- -D warnings   # lints; CI denies warnings
cargo run -p jellysink -- run                           # the daemon (default subcommand)
cargo run -p jellytui                                   # the terminal frontend
```

A workspace of three crates.
- `crates/core` is the library (`jellysink-core`): everything both binaries touch.
- `crates/jellysink` and `crates/jellytui` are plain **bin** crates over it — neither has a `lib.rs`, so everything inside them stays `pub(crate)` and `dead_code` covers all of it. `default-members` is unset, so a bare `cargo build` or `cargo test` still covers all three; the `-p` in the run commands is only because the workspace has two binaries.

`core` is the only crate with a `pub` surface, and it is kept to what a binary actually names: anything neither binary uses is `pub(crate)` there too, or `dead_code` stops working in core

## Architecture

Design notes for the trickier subsystems live in `specs/`:

## Conventions

Everything below is a rule `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` cannot express. Those two already cover formatting and lints; this section is the rest, and it is what review checks.

### Comments

- **The default is none.** Write one only where a competent Rust reader would still be guessing — a non-obvious constraint, an invariant, the reason the obvious approach was rejected. Simple code gets no comment at all; "say why" is not a licence to justify something self-evident, and a doc comment is not owed to every item.
- Never say a thing twice. If an assert message, error string, function name or test name already carries it, the comment gets deleted, not reworded. Never narrate what the next line does.
- Never list your own call sites, or any other inventory of the code as it stands today — it is wrong the moment someone adds a fourth one. Invariants age well; inventories do not.
- **Write the present, not the change.** What a thing used to do, what was removed, and when it changed belong to git; a reader arriving without that history reads them as claims about the code in front of them. "X once did Y, now it does Z" is just "X does Z" — delete the first clause, do not soften it. This binds `specs/` exactly as it binds a comment: a spec edited alongside a change states the invariant that now holds rather than narrating the edit, and the diff and commit message carry the rest. Two things are not history and stay: the reason a plausible alternative was **rejected**, and a test comment naming the **regression it guards** (`mpv/ipc_test.rs`, `runtime/queue/stubs_test.rs`), which is that test's whole reason to exist.
- **Keep it extremely short: two lines is the ceiling, doc comments included.** One sentence is the target. A third line is justified only when a single invariant genuinely does not fit in two and every word of it is that invariant. The failure shape is a comment that names the constraint and then keeps going — a second clause restating the first, an aside about which endpoint the data came from, a consequence the reader can derive. Delete the continuation and keep the first sentence; that is a cut, not a rewrite. Anything still too long for two lines belongs in `specs/` or the commit message, not above the item.
- The inverse is a defect too, but only where you can name the thing a reader would be guessing about: an invariant or a rejected alternative left unwritten.

### Names

- Names spell things out: `audio_stream_id`, not `aid`; `subtitle_index`, not `sidx`. Abbreviate only where the short form *is* the domain term (mpv's own `sid`/`aid` properties, `ipc`, `url`), and take the full name the moment the value crosses out of that boundary into our own code.
- Conventional short bindings for errors, contexts, connections and iterator variables (`e`, `ctx`, `conn`, `iface`) are fine. What is not is an abbreviation of a field or type the codebase spells out elsewhere — `np` for a `now_playing` field, `prep` for a `PreparedPlay`. That contrast is the test.

### Errors

- `color_eyre` (`eyre::Result`), `wrap_err`/`wrap_err_with` with context that names the resource that failed — the path, the URL, the socket. A bare `?` on a filesystem, network or IPC call loses it. `thiserror` only for typed error enums.
- CLI mistakes (not logged in, already running, unknown config key) use `usage_err` (`UsageError`) — the binary prints the message and exits 1 without a color-eyre dump. A user-facing CLI error raised as a plain `eyre!` is wrong.
- Matching the adjacent function is a partial defence, not a full one. If the surrounding code already drops the path, the new code still carries it.
- **A discarded error is the judgement call that comes up most.** Every `let _ = …`, `if let Ok(..)` and ignored `Result` is one of two things. Correct, where fail-open is the stated policy — the tray and MPRIS document that no session bus is a warning and not a fatal error, and a send on a channel whose receiver is gone is often genuinely nothing. Or a defect, where the daemon is the only thing that will ever see the failure and it keeps no record of it: a dropped write or serialize error with no `tracing` call leaves the operator with nothing, and often makes the *other* end report the wrong failure. The second kind gets a `warn!`.

### Logging

`tracing` macros in daemon code, never `println!` there; `println!` is for CLI subcommand output only.

### Tests

- **Every test needs a reason to exist.** Don't assert that a constant still holds its value, that an enum still has its variants, that a constructor assigned its arguments, or that a derived `Serialize`/`Deserialize` round-trips with no serde attributes pinning anything down — the compiler already says all of that, and such a test only ever breaks when someone edits it. Test behaviour in a realistic scenario instead (feed a real payload through the parser, drive the state machine to the edge case), or pin the thing that is actually a contract, such as the serialized field names `status.rs` promises the other binary.
- Tests live in a sibling file next to the one they test: `streams.rs` → `streams_test.rs`, `mod.rs` → `mod_test.rs`. The file under test ends with the three-line declaration
  ```rust
  #[cfg(test)]
  #[path = "streams_test.rs"]
  mod tests;
  ```
  `#[path]` rather than a plain sibling `mod` in the parent `mod.rs`, because this keeps `tests` a *child* module and most test modules read their parent's private items (`config::parse_mpv_args`, `PlaylistWindow`'s private `queue` field, …). Test paths are unchanged by this (`media::streams::tests::…`), so `cargo test <filter>` works as before. `tempfile::TempDir` for anything touching the filesystem. A `mod tests` inline in the file under test is wrong.
- **A test that answers for an external program rather than for us belongs in its own identifiable file**, so a failure in an environment lacking that program lands in one module. `crates/jellysink/src/mpv/integration_test.rs` (`mpv::integration_tests`) is the established instance: it drives a real player, and is where mpv behaviour we depend on but cannot fake belongs — `#EXTINF` titles surviving a `loadlist`, `insert-at` not moving the playing entry, `sid`/`aid` observers firing, and which `end-file` reason each way of ending a file produces. It plays `crates/jellysink/tests/fixtures/sample.mkv` (3 s, two audio and two subtitle tracks, 31 KB, regenerate with the `make-fixtures.sh` beside it). Add a case here when a bug turns out to be mpv doing something other than what we assumed. `crates/jellysink/src/daemon/mpris_integration_test.rs` (`daemon::mpris::integration_tests`) is the same shape for a live D-Bus session; any other external daemon wants the same treatment.
- **No sleep-based synchronization.** A bare `sleep(50ms)` before an assert, or a `while !path.exists()` poll loop, is flaky by construction, and a green run has to mean the tests ran. Wait on a channel, a readiness signal, or a bounded retry that fails for real at the end.
- Always look if there is not a test already covering the behaviour you are about to add. If so, extend, not duplicate.

### Maintainability

If you think that you can simplify a piece of code, ask the user to do so. We want to keep the codebase as small and maintainable as possible.

### Secrets and the filesystem

- Config and credential files are written atomically (tmp file + rename); cred.json is mode 0600. The config directory itself is 0700 — `mpv.sock` lives there, it is created by mpv (so we cannot pick its mode), and `http-header-fields` on it hands out the access token.
- The file modes are only half of it. Check **every channel the access token can leave the process by** — a D-Bus property, a JSON payload on `stop.sock`, a log line, a URL handed to another program. `redact_api_key` exists for this, and redacting the token on one path while broadcasting it unredacted on another is the inconsistency to catch.

### Specs

Read the spec that covers a file before touching it (`specs/playlist.md`, `specs/tracks.md`, `specs/tui.md`, `specs/session.md` — see Architecture for which covers what), **and also whenever you add a `tokio::spawn` or a `select!` arm anywhere in the daemon**, wherever the spawning code lives: `specs/session.md` documents the task and channel contract for the whole daemon, not just for `runtime/session.rs`, and the file that breaks it is often somewhere else entirely.

Two ways to break one, both of which mean editing the spec in the same commit:

- The change **contradicts** a documented invariant. Every spawned task being wrapped in an `AbortOnDrop` (`runtime/task.rs`) is one such invariant — a detached `tokio::spawn` either violates it or needs the spec amended to record the deliberate exception and why.
- The change makes an **inventory** stale. `specs/session.md` carries a task/channel table and a `select!`-arm table; adding a channel, task or arm leaves them silently wrong without contradicting anything. Update the row.
