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

`cargo test` includes `src/mpv/integration_test.rs`, which spawns a real mpv (headless: `--no-config --vo=null --ao=null`) and drives it over its real IPC socket, so **mpv must be installed to run the suite** — without it those tests fail rather than skip, because a green run has to mean they ran. CI's test job installs it.

Note: `mpv::tests::ipc_roundtrip_against_fake_socket` and everything in `mpv::integration_tests` create Unix domain sockets and fail with `PermissionDenied` in sandboxes that block socket creation. They pass on a normal machine — don't "fix" them for sandbox environments.

Release tags are `X.Y.Z` with **no** `v` prefix and must match `Cargo.toml` `version`. GitHub Actions (`.github/workflows/`) runs fmt/clippy/test and musl release builds (x86_64 and aarch64) on push and PR. A matching tag re-runs fmt/clippy/test, rebuilds those binaries, and publishes the GitHub release — a tag on a commit that would fail CI does not ship.

Lint policy lives in `Cargo.toml`'s `[lints]` table, so a local `cargo clippy` matches CI. Everything outside the four items `main.rs` imports (`UsageError`, `app::cli`, `app::config::{Config, Paths}`, `app::tracing::init_tracing`) is `pub(crate)` — keep it that way, or `dead_code` stops working crate-wide.

## Architecture

Design notes for the trickier subsystems live in `specs/`:

- `specs/playlist.md` — the queue/mpv window invariant, when episode data is
  fetched, and why the prepend is split into two phases. **Read this before
  touching `src/runtime/window.rs`, `src/runtime/queue.rs` or
  `src/runtime/playback.rs`**; the index arithmetic has subtle invariants that
  are easy to break. `PlaylistWindow` (`src/runtime/window.rs`) owns all of it —
  keep its fields private and add a method rather than reaching past them.
- `specs/tracks.md` — the Jellyfin-index ↔ mpv-track-id maps, how a hand-picked
  track is remembered as an identity and re-matched in the next episode, and the
  `TrackState::settled` baseline that makes stale property changes no-ops.
  **Read this before touching `src/media/track.rs`, `src/media/streams.rs` or
  the `configure_streams` / `adopt_mpv_track` paths in
  `src/runtime/playback.rs`**; the two numbering systems and the two subtitle
  counter gates look interchangeable and are not.
- `specs/session.md` — the daemon loop: task and channel ownership, reconnect
  backoff, keepalive, report ordering, mpv generations, the latching `Signal`,
  and the `transitioning` / `stopping` contract behind `end_file_action`.
  **Read this before touching `src/runtime/session.rs`, `src/app/signal.rs` or
  `src/report.rs`**, and before adding a `select!` arm or a spawned task to a
  session.

Single binary crate. `src/main.rs` parses the CLI (clap derive) and dispatches; `src/lib.rs` re-exports the modules:

- `app/` — daemon/CLI plumbing, none of it playback logic: `cli.rs` (subcommand implementations — `login`, `logout`, `config`, `run`, `stop`, `update`; CLI `update` stops a running daemon, tray **Install update** opens a terminal (`update --from-tray`) and restarts in place), `config.rs` (`Paths` — config dir + file locations, `Config` — config.toml, `MpvArgs` — mpv_args.conf, re-read on every mpv spawn, `Credentials` — cred.json, mode 0600, `normalize_server_url` — bare host → `http://host:8096`), `instance.rs` (single-instance lock and the `stop.sock` socket used by `jellysink stop` (`stop`) and tray update (`restart`)), `signal.rs` (`Signal`, a latching `watch`-channel signal used for shutdown/restart — see `specs/session.md`), `terminal.rs` (pick a terminal emulator — `xdg-terminal-exec`, `$TERMINAL`, then a known list — and spawn a command in it), `tracing.rs` (`init_tracing`; `log_level` from config, overridden by `RUST_LOG` when set; uses `Targets` (not `EnvFilter`) to keep the binary small: measured at +186 KB / +2.1% for `env-filter`, even though `self_update` already links `regex` and the crate count barely moves — the trade-off is no span-field filtering; `validate_log_level` rejects a bare word that is not a level, because `Targets` would otherwise read it as a target name and silence everything), `tray.rs` (optional StatusNotifier tray icon (ksni) with a quit item; shows Install update when a newer GitHub release exists), `update.rs` (GitHub Releases self-update (`self_update`): check on daemon start, install via tray or `jellysink update`). `cli`, `config` and `tracing` are `pub` (reached from `main.rs` as `jellysink::app::cli`, `jellysink::app::config::{Config, Paths}`, `jellysink::app::tracing::init_tracing`); the rest of `app/` is `pub(crate)`.
- `cast.rs` — `CastEvent` enum: the Jellyfin remote-control commands (PlayNow/Pause/Seek/…) parsed from WebSocket messages. Parsing only.
- `jellyfin/` — server API: `auth.rs` (login, `Api` client, cached auth header, `AuthExpired`), `playback.rs` (playback info / session endpoints, as further `impl Api`), `profile.rs` (device profile that requests DirectPlay; `PlayableMediaTypes` is video only), `session.rs` (WebSocket URL + message parsing), `encode_query_value` in `mod.rs`.
- `media/` — turns a Jellyfin item into a prepared mpv play: `mod.rs` (`PreparedPlay`, `PlayRequest`, `prepare_play`, `select_media_source`), `streams.rs` (the `PlaybackInfo`/`MediaSource`/`MediaStream` serde models, the Jellyfin-index ↔ mpv-track-id maps, and the `SubtitleId`/`AudioId` identity lists), `track.rs` (the shared matcher: remembering the last hand-picked track and re-finding it in the next episode by language and track name — stream indexes are per-file, so the index alone means nothing across episodes; the remembered choice lives in `Runtime`'s `audio`/`subtitle` `TrackState` fields, in memory only and never persisted; a track picked in the mpv window counts as hand-picked too — `mpv/` observes `sid`/`aid` and `runtime::playback::adopt_mpv_track` maps those back to Jellyfin stream indexes), `subtitle.rs` and `audio.rs` (the two thin sides of it — names and log wording only), `title.rs` (display titles).
- `ticks.rs` — Jellyfin position ticks (100 ns units) ↔ seconds. Used by `runtime/`, nothing to do with media.
- `mpv/` — `mod.rs`: `MpvSession`, spawns mpv with `--input-ipc-server`, `--force-window=yes`, `--idle=yes` (never `vo`/`hwdec`/`scale`/`glsl-shaders`, never `--no-config`), speaks JSON IPC over the Unix socket, exposes typed helpers (`loadfile`, `pause`, `seek_absolute`, …) and `MpvEvent`. `observe_subtitle_track` and `observe_audio_track` register the two `observe_property` calls we make; `MpvEvent::SubtitleTrackChanged` and `MpvEvent::AudioTrackChanged` deliberately carry no value, because property changes are handled long after they are emitted and `Runtime` re-reads `sid`/`aid` instead (`TrackState::settled` is what separates a user's pick from mpv's own auto-selection). `mod_test.rs` is the ordinary unit-test sibling; `integration_test.rs` is the real-mpv suite, see below.
- `runtime/` — the daemon loop: `mod.rs` reconnect/backoff — one `Runtime` is built in `run` and outlives every WebSocket session, so a reconnect is non-destructive: mpv keeps playing, the queue, volume and remembered tracks stay put, and only the socket, its reader and the keepalive are per-session (`run_session` re-announces the current play once reconnected). `playback.rs` applies `CastEvent`s to mpv and reports state back — audio and subtitles go through one `TrackKind`-parameterised path (`apply_track`, `adopt_mpv_track`, `remember_track`, `settle_track`) rather than two copies of it, `queue.rs` series autoplay (next episode in aired order, appended to mpv's playlist), `window.rs` the `PlaylistWindow` invariant (queue + how much of it mpv holds) plus the mpv playlist semantics that go with it — `Queue`, `end_file_action`, `playlist_eof`, `queue_index_at`, `ignore_stop_for_playlist`.
- `report.rs` — reports playback state back to the Jellyfin session.

Also in the tree (not a Rust module): `systemd/jellysink.service` — user unit (`WantedBy=graphical-session.target`); `ExecStart=%h/.local/bin/jellysink`.

## Conventions

- Errors: `color_eyre` (`eyre::Result`), `wrap_err`/`wrap_err_with` with path-bearing context; `thiserror` only for typed error enums. CLI mistakes (not logged in, already running, unknown config key) use `usage_err` (`UsageError`) — the binary prints the message and exits 1 without a color-eyre dump.
- Async: tokio; I/O is async except small config file reads.
- Logging: `tracing` macros, never `println!` in daemon code (CLI output uses `println!`).
- Config/credential files are written atomically (tmp file + rename); cred.json is mode 0600. The config directory itself is 0700 — `mpv.sock` lives there, it is created by mpv (so we cannot pick its mode), and `http-header-fields` on it hands out the access token.
- Comments: **the default is none.** Write one only where a competent Rust reader would still be guessing — a non-obvious constraint, an invariant, the reason the obvious approach was rejected. Simple code gets no comment at all; "say why" is not a licence to justify something self-evident, and a doc comment is not owed to every item.
- Never say a thing twice. If an assert message, error string, function name or test name already carries it, the comment gets deleted, not reworded. Never narrate what the next line does.
- One or two lines is the ceiling, not a target. Anything that needs a paragraph belongs in `specs/` or the commit message.
- Names spell things out: `audio_stream_id`, not `aid`; `subtitle_index`, not `sidx`. Abbreviate only where the short form *is* the domain term (mpv's own `sid`/`aid` properties, `ipc`, `url`), and keep the full name the moment the value crosses into our own code.
- Every test needs a reason to exist. Don't assert that a constant still holds its value or that an enum still has its variants — the compiler already says that, and such a test only breaks when someone edits it. Test behaviour in a realistic scenario instead: feed a real payload through the parser, drive the state machine to the edge case, check what the code *does* with the constant.
- Tests live in a sibling file next to the one they test: `streams.rs` → `streams_test.rs`, `mod.rs` → `mod_test.rs`. The file under test ends with the three-line declaration
  ```rust
  #[cfg(test)]
  #[path = "streams_test.rs"]
  mod tests;
  ```
  `#[path]` rather than a plain sibling `mod` in the parent `mod.rs`, because this keeps `tests` a *child* module and most test modules read their parent's private items (`config::parse_mpv_args`, `PlaylistWindow`'s private `queue` field, …). Test paths are unchanged by this (`media::streams::tests::…`), so `cargo test <filter>` works as before. `tempfile::TempDir` for anything touching the filesystem.
- `src/mpv/integration_test.rs` (`mpv::integration_tests`) is the exception that answers for mpv rather than for us: it drives a real player, and is where mpv behaviour we depend on but cannot fake belongs — `#EXTINF` titles surviving a `loadlist`, `insert-at` not moving the playing entry, `sid`/`aid` observers firing, and which `end-file` reason each way of ending a file produces. It plays `tests/fixtures/sample.mkv` (3 s, two audio and two subtitle tracks, 31 KB, regenerate with `tests/fixtures/make-fixtures.sh`). Add a case here when a bug turns out to be mpv doing something other than what we assumed.
- Keep the DirectPlay/no-transcode and user-mpv-config guarantees (see README "What it will not do") — they are the product's core promises.
