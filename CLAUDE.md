# CLAUDE.md

## Project

jellysink is a headless Jellyfin cast target for Linux, written in Rust (edition 2024). It registers as a remote player, receives Play commands over a WebSocket, and DirectPlays the stream in the user's installed `mpv`.

## Commands

```bash
cargo build                                             # both binaries
testserver/start.sh                                     # the Jellyfin the tests need (docker, or CONTAINER=podman)
cargo nextest run                                       # the whole workspace (CI runs this, not `cargo test`)
testserver/stop.sh                                      # remove that server
cargo fmt                                               # `cargo fmt --check` must stay clean
cargo clippy --workspace --all-targets -- -D warnings   # CI denies warnings
cargo run -p jellysink -- run                           # the daemon (default subcommand)
cargo run -p jellytui                                   # the terminal frontend
```

## Workspace

- **`crates/core`** (`jellysink-core`) is the library holding everything both binaries touch.
- **`crates/jellysink` and `crates/jellytui` are bin-only crates.** Neither has a `lib.rs`, so everything inside them stays `pub(crate)` and `dead_code` covers all of it.
- **`core`'s `pub` surface is only what a binary actually names.** Everything else is `pub(crate)`, or `dead_code` stops working there.

## Specs

Design notes live in `specs/`: `playlist.md` (queue, episode fetching, handing it to mpv), `tracks.md` (audio/subtitle mapping), `tui.md` (jellytui), `session.md` (daemon wiring, tasks and channels).

- **Read the spec covering a file before touching it.** Also read `specs/session.md` whenever you add a `tokio::spawn` or `select!` arm anywhere in the daemon; it documents the task and channel contract for the whole daemon.
- **A change that contradicts a documented invariant edits the spec in the same commit.** For example, every spawned task is wrapped in an `AbortOnDrop` (`runtime/task.rs`); a detached `tokio::spawn` needs the spec to record the exception and why.
- **A change that makes an inventory stale updates it.** `specs/session.md` carries a task/channel table and a `select!`-arm table; a new channel, task or arm gets its row.

## Comments

- **Write the present, not the change.** What a thing used to do or what was removed belongs to git; "X once did Y, now it does Z" is just "X does Z". This binds `specs/` too.
- **Two kinds of history stay:** the reason a plausible alternative was rejected, and a test comment naming the regression it guards (`mpv/ipc_test.rs`, `runtime/queue/stubs_test.rs`).
- **Two lines is the ceiling, doc comments included.** If a comment names the constraint and then keeps going, cut the continuation; anything longer belongs in `specs/` or the commit message.
- **A missing comment is a defect only when you can name what the reader would guess about:** an unwritten invariant or rejected alternative.

## Naming

- **Abbreviate only where the short form is the domain term** (mpv's `sid`/`aid`, `ipc`, `url`), and take the full name once the value leaves that boundary.
- **Conventional short bindings are fine** (`e`, `ctx`, `conn`, `iface`); abbreviating a field or type spelled out elsewhere (`np` for `now_playing`, `prep` for `PreparedPlay`) is not.

## Errors

- **`color_eyre` with context naming the failed resource.** Use `wrap_err`/`wrap_err_with` with the path, URL or socket; a bare `?` on filesystem, network or IPC calls loses it, even if the adjacent code does the same. `thiserror` only for typed error enums.
- **CLI mistakes use `usage_err` (`UsageError`).** The binary prints the message and exits 1 without a color-eyre dump; a plain `eyre!` there is wrong.
- **Every discarded error (`let _ =`, `if let Ok(..)`, ignored `Result`) is either policy or a defect.** Fail-open is fine where documented (no session bus for tray/MPRIS, a send to a dropped receiver); a dropped failure only the daemon would ever see gets a `warn!`.

## Logging

- **`tracing` in daemon code, never `println!`.** `println!` is for CLI subcommand output only.

## Tests

- **Don't test what the compiler already guarantees.** No asserting constants, enum variants, constructor assignments or plain derived serde round-trips; pin real contracts instead, such as the serialized field names `status.rs` promises the other binary.
- **Tests live in a sibling `_test.rs` file** (`streams.rs` → `streams_test.rs`, `mod.rs` → `mod_test.rs`), declared at the end of the file under test with:
  ```rust
  #[cfg(test)]
  #[path = "streams_test.rs"]
  mod tests;
  ```
  This keeps `tests` a child module so it can read its parent's private items. An inline `mod tests` is wrong.
- **`tempfile::TempDir` for anything touching the filesystem.**
- **Tests that answer for an external program go in their own file**, so a missing program fails one module:
  - `crates/jellysink/src/mpv/integration_test.rs` — real mpv behaviour we depend on but cannot fake; add a case when a bug turns out to be mpv not doing what we assumed. Plays `crates/jellysink/tests/fixtures/sample.mkv` (regenerate with `make-fixtures.sh` beside it).
  - `crates/jellysink/src/daemon/mpris_integration_test.rs` — a live D-Bus session.
  - `crates/core/src/jellyfin/integration_test.rs` (`Api` calls) and `crates/jellysink/src/runtime/jellyfin_integration_test.rs` (daemon end to end) — against the `testserver/start.sh` server (Jellyfin 12.1, one series of 106 episodes from `testserver/episodes.tsv`).
- **Bounded waits only.** Wait on a channel, readiness signal, or a retry that fails for real at the end; never a bare sleep or an unbounded `while !path.exists()` poll.
- **Extend an existing test before writing a new one** covering the same behaviour.

## Secrets and the filesystem

- **Config and credential files are written atomically** (tmp file + rename); `cred.json` is 0600 and the config directory is 0700, since mpv creates `mpv.sock` there and `http-header-fields` on it hands out the access token.
- **Check every channel the access token can leave by** — a D-Bus property, a `control.sock` frame, a log line, a URL handed to another program. Use `redact_api_key`, consistently on every path.

## Checks

- **Run `.claude/skills/drive-jellytui` after the cargo tests pass** to verify the change in the real TUI and that nothing else broke.
