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

Note: `mpv::tests::ipc_roundtrip_against_fake_socket` creates a Unix domain socket and fails with `PermissionDenied` in sandboxes that block socket creation. It passes on a normal machine — don't "fix" it for sandbox environments.

Release tags are `X.Y.Z` with **no** `v` prefix and must match `Cargo.toml` `version`. GitHub Actions (`.github/workflows/`) runs fmt/clippy/test and musl release builds (x86_64 and aarch64) on push and PR. A matching tag rebuilds those binaries and publishes the GitHub release.

## Architecture

Single binary crate. `src/main.rs` parses the CLI (clap derive) and dispatches; `src/lib.rs` re-exports the modules:

- `cli.rs` — subcommand implementations (`login`, `logout`, `config`, `run`, `stop`, `update`). CLI `update` stops a running daemon; tray **Install update** restarts in place.
- `config.rs` — `Paths` (config dir + file locations), `Config` (config.toml), `MpvArgs` (mpv_args.conf, re-read on every mpv spawn), `Credentials` (cred.json, mode 0600), `normalize_server_url` (bare host → `http://host:8096`).
- `instance.rs` — single-instance lock and the `stop.sock` socket used by `jellysink stop`.
- `cast.rs` — `CastEvent` enum: the Jellyfin remote-control commands (PlayNow/Pause/Seek/…) parsed from WebSocket messages. Also `Queue` and playlist-EOF helpers (`end_file_action`, `playlist_eof`) so mpv `keep-open` and user Next do not skip an episode.
- `jellyfin/` — server API: `auth.rs` (login, `Api` client, auth header), `playback.rs` (playback info / endpoints), `profile.rs` (device profile that requests DirectPlay; `PlayableMediaTypes` is video only), `session.rs` (WebSocket URL + message parsing).
- `media.rs` — turns a Jellyfin item into a prepared mpv play (URL, title, audio/sub track selection).
- `mpv.rs` — `MpvSession`: spawns mpv with `--input-ipc-server`, `--force-window=yes`, `--idle=yes` (never `vo`/`hwdec`/`scale`/`glsl-shaders`, never `--no-config`), speaks JSON IPC over the Unix socket, exposes typed helpers (`loadfile`, `pause`, `seek_absolute`, …) and `MpvEvent`.
- `runtime/` — the daemon loop: `mod.rs` reconnect/backoff, `playback.rs` applies `CastEvent`s to mpv and reports state back, `queue.rs` series autoplay (next episode in aired order, appended to mpv's playlist).
- `report.rs` — reports playback state back to the Jellyfin session.
- `tracing.rs` — `init_tracing`; `log_level` from config, overridden by `RUST_LOG` when set. Uses `Targets` (not `EnvFilter`) to keep the binary small.
- `tray.rs` — optional StatusNotifier tray icon (ksni) with a quit item; shows Install update when a newer GitHub release exists.
- `update.rs` — GitHub Releases self-update (`self_update`): check on daemon start, install via tray or `jellysink update`.

Also in the tree (not a Rust module): `systemd/jellysink.service` — user unit (`WantedBy=graphical-session.target`); `ExecStart=%h/.local/bin/jellysink`.

## Conventions

- Errors: `color_eyre` (`eyre::Result`), `wrap_err`/`wrap_err_with` with path-bearing context; `thiserror` only for typed error enums. CLI mistakes (not logged in, already running, unknown config key) use `usage_err` (`UsageError`) — the binary prints the message and exits 1 without a color-eyre dump.
- Async: tokio; I/O is async except small config file reads.
- Logging: `tracing` macros, never `println!` in daemon code (CLI output uses `println!`).
- Config/credential files are written atomically (tmp file + rename); cred.json is mode 0600.
- Tests live in `#[cfg(test)] mod tests` at the bottom of the file they test; `tempfile::TempDir` for anything touching the filesystem.
- Keep the DirectPlay/no-transcode and user-mpv-config guarantees (see README "What it will not do") — they are the product's core promises.
