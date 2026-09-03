# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project

jellysink is a headless Jellyfin cast target for Linux, written in Rust (edition 2024). It registers as a remote player, receives Play commands over a WebSocket, and DirectPlays the stream in the user's installed mpv — deliberately never setting `vo`, `hwdec`, `scale`, or `glsl-shaders` and never passing `--no-config`, so the user's own mpv config and upscalers apply. It does not transcode: if the server won't DirectPlay/DirectStream, playback is refused.

## Commands

```bash
cargo build                                 # build
cargo test                                  # run all tests
cargo fmt                                   # format; `cargo fmt --check` must stay clean
cargo clippy --all-targets -- -D warnings   # lints; CI denies warnings
cargo run -- run                            # the daemon (default subcommand)
```

Global CLI flag: `--config DIR` (default `~/.config/jellysink`).

Every build is a musl build. `.cargo/config.toml` sets `build.target = "x86_64-unknown-linux-musl"`, and `build.rs` panics on any non-musl target, so a glibc build cannot happen by accident — releases, `install.sh`, and the self-updater all ship static musl binaries, and a dynamically linked glibc build is not what any user runs. Artifacts therefore live under `target/x86_64-unknown-linux-musl/`, not `target/debug/`. Requires `rustup target add x86_64-unknown-linux-musl` plus a musl C compiler for jemalloc (`musl` on Arch, `musl-tools` on Debian). On an aarch64 host, `export CARGO_BUILD_TARGET=aarch64-unknown-linux-musl`. Don't add `--target x86_64-unknown-linux-gnu` to work around a build error — fix the error.

Note: `mpv::tests::ipc_roundtrip_against_fake_socket` creates a Unix domain socket and fails with `PermissionDenied` in sandboxes that block socket creation. It passes on a normal machine — don't "fix" it for sandbox environments.

Release tags are `X.Y.Z` with **no** `v` prefix and must match `Cargo.toml` `version`. GitHub Actions (`.github/workflows/`) runs fmt/clippy/test and musl release builds (x86_64 and aarch64) on push and PR. A matching tag re-runs fmt/clippy/test, rebuilds those binaries, and publishes the GitHub release — a tag on a commit that would fail CI does not ship.

Lint policy lives in `Cargo.toml`'s `[lints]` table, so a local `cargo clippy` matches CI. Everything outside the four items `main.rs` imports (`UsageError`, `cli`, `config::{Config, Paths}`, `tracing::init_tracing`) is `pub(crate)` — keep it that way, or `dead_code` stops working crate-wide.

## Architecture

Design notes for the trickier subsystems live in `specs/`:

- `specs/playlist.md` — the queue/mpv window invariant, when episode data is
  fetched, and why the prepend is split into two phases. **Read this before
  touching `src/runtime/window.rs`, `src/runtime/queue.rs` or
  `src/runtime/playback.rs`**; the index arithmetic has subtle invariants that
  are easy to break. `PlaylistWindow` (`src/runtime/window.rs`) owns all of it —
  keep its fields private and add a method rather than reaching past them.

Single binary crate. `src/main.rs` parses the CLI (clap derive) and dispatches; `src/lib.rs` re-exports the modules:

- `cli.rs` — subcommand implementations (`login`, `logout`, `config`, `run`, `stop`, `update`). CLI `update` stops a running daemon; tray **Install update** opens a terminal (`update --from-tray`) and restarts in place.
- `config.rs` — `Paths` (config dir + file locations), `Config` (config.toml), `MpvArgs` (mpv_args.conf, re-read on every mpv spawn), `Credentials` (cred.json, mode 0600), `normalize_server_url` (bare host → `http://host:8096`).
- `instance.rs` — single-instance lock and the `stop.sock` socket used by `jellysink stop` (`stop`) and tray update (`restart`).
- `cast.rs` — `CastEvent` enum: the Jellyfin remote-control commands (PlayNow/Pause/Seek/…) parsed from WebSocket messages. Parsing only.
- `jellyfin/` — server API: `auth.rs` (login, `Api` client, cached auth header, `AuthExpired`), `playback.rs` (playback info / session endpoints, as further `impl Api`), `profile.rs` (device profile that requests DirectPlay; `PlayableMediaTypes` is video only), `session.rs` (WebSocket URL + message parsing), `encode_query_value` in `mod.rs`.
- `media/` — turns a Jellyfin item into a prepared mpv play: `mod.rs` (`PreparedPlay`, `PlayRequest`, `prepare_play`, `select_media_source`), `streams.rs` (the `PlaybackInfo`/`MediaSource`/`MediaStream` serde models and the Jellyfin-index ↔ mpv-track-id maps), `title.rs` (display titles).
- `ticks.rs` — Jellyfin position ticks (100 ns units) ↔ seconds. Used by `runtime/`, nothing to do with media.
- `mpv.rs` — `MpvSession`: spawns mpv with `--input-ipc-server`, `--force-window=yes`, `--idle=yes` (never `vo`/`hwdec`/`scale`/`glsl-shaders`, never `--no-config`), speaks JSON IPC over the Unix socket, exposes typed helpers (`loadfile`, `pause`, `seek_absolute`, …) and `MpvEvent`.
- `runtime/` — the daemon loop: `mod.rs` reconnect/backoff, `playback.rs` applies `CastEvent`s to mpv and reports state back, `queue.rs` series autoplay (next episode in aired order, appended to mpv's playlist), `window.rs` the `PlaylistWindow` invariant (queue + how much of it mpv holds) plus the mpv playlist semantics that go with it — `Queue`, `end_file_action`, `playlist_eof`, `queue_index_at`, `ignore_stop_for_playlist`.
- `report.rs` — reports playback state back to the Jellyfin session.
- `tracing.rs` — `init_tracing`; `log_level` from config, overridden by `RUST_LOG` when set. Uses `Targets` (not `EnvFilter`) to keep the binary small: measured at +186 KB / +2.1% for `env-filter`, even though `self_update` already links `regex` and the crate count barely moves. The trade-off is no span-field filtering. `validate_log_level` rejects a bare word that is not a level, because `Targets` would otherwise read it as a target name and silence everything.
- `terminal.rs` — pick a terminal emulator (`xdg-terminal-exec`, `$TERMINAL`, then a known list) and spawn a command in it.
- `tray.rs` — optional StatusNotifier tray icon (ksni) with a quit item; shows Install update when a newer GitHub release exists.
- `update.rs` — GitHub Releases self-update (`self_update`): check on daemon start, install via tray or `jellysink update`.

Also in the tree (not a Rust module): `systemd/jellysink.service` — user unit (`WantedBy=graphical-session.target`); `ExecStart=%h/.local/bin/jellysink`.

## Conventions

- Errors: `color_eyre` (`eyre::Result`), `wrap_err`/`wrap_err_with` with path-bearing context; `thiserror` only for typed error enums. CLI mistakes (not logged in, already running, unknown config key) use `usage_err` (`UsageError`) — the binary prints the message and exits 1 without a color-eyre dump.
- Async: tokio; I/O is async except small config file reads.
- Logging: `tracing` macros, never `println!` in daemon code (CLI output uses `println!`).
- Config/credential files are written atomically (tmp file + rename); cred.json is mode 0600. The config directory itself is 0700 — `mpv.sock` lives there, it is created by mpv (so we cannot pick its mode), and `http-header-fields` on it hands out the access token.
- Tests live in `#[cfg(test)] mod tests` at the bottom of the file they test; `tempfile::TempDir` for anything touching the filesystem.
- Keep the DirectPlay/no-transcode and user-mpv-config guarantees (see README "What it will not do") — they are the product's core promises.
