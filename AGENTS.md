# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project

jellysink is a headless Jellyfin cast target for Linux, written in Rust (edition 2024). It registers as a remote player, receives Play commands over a WebSocket, and DirectPlays the stream in the user's installed mpv — deliberately never setting `vo`, `hwdec`, `scale`, or `glsl-shaders` and never passing `--no-config`, so the user's own mpv config and upscalers apply. It does not transcode: if the server won't DirectPlay/DirectStream, playback is refused.

## Commands

```bash
cargo build                                             # both binaries
cargo test                                              # the whole workspace
cargo fmt                                               # format; `cargo fmt --check` must stay clean
cargo clippy --workspace --all-targets -- -D warnings   # lints; CI denies warnings
cargo run -p jellysink -- run                           # the daemon (default subcommand)
cargo run -p jellytui                                   # the terminal frontend
```

A workspace of three crates. `crates/core` is the library (`jellysink-core`): everything both binaries touch. `crates/jellysink` and `crates/jellytui` are plain **bin** crates over it — neither has a `lib.rs`, so everything inside them stays `pub(crate)` and `dead_code` covers all of it. `default-members` is unset, so a bare `cargo build` or `cargo test` still covers all three; the `-p` in the run commands is only because the workspace has two binaries.

The boundary is the point: `jellysink` does not compile `ratatui`, `ratatui-image` or `image`, and `jellytui` compiles none of `ksni`, `zbus`, `self_update`, `tokio-tungstenite`, `dialoguer` or jemalloc. Keep it that way — if a new dependency has to go in `core`, both binaries pay for it.

Global CLI flag: `--config DIR` (default `~/.config/jellysink`).

Every build is a musl build. `.cargo/config.toml` sets `build.target = "x86_64-unknown-linux-musl"`, and `crates/core/build.rs` panics on any non-musl target (one build script, and both binaries depend on core), so a glibc build cannot happen by accident — releases, `install.sh`, and the self-updater all ship static musl binaries, and a dynamically linked glibc build is not what any user runs. Artifacts therefore live under `target/x86_64-unknown-linux-musl/`, not `target/debug/`. Requires `rustup target add x86_64-unknown-linux-musl` plus a musl C compiler for jemalloc (`musl` on Arch, `musl-tools` on Debian). On an aarch64 host, `export CARGO_BUILD_TARGET=aarch64-unknown-linux-musl`. Don't add `--target x86_64-unknown-linux-gnu` to work around a build error — fix the error.

`cargo test` includes `crates/jellysink/src/mpv/integration_test.rs`, which spawns a real mpv (headless: `--no-config --vo=null --ao=null`) and drives it over its real IPC socket, so **mpv must be installed to run the suite** — without it those tests fail rather than skip, because a green run has to mean they ran. CI's test job installs it.

Note: `mpv::tests::ipc_roundtrip_against_fake_socket` and everything in `mpv::integration_tests` create Unix domain sockets and fail with `PermissionDenied` in sandboxes that block socket creation. They pass on a normal machine — don't "fix" them for sandbox environments.

Release tags are `X.Y.Z` with **no** `v` prefix and must match the root `Cargo.toml`'s `[workspace.package] version`, which all three crates inherit. GitHub Actions (`.github/workflows/`) runs fmt/clippy/test and musl release builds (x86_64 and aarch64) on push and PR. A matching tag re-runs fmt/clippy/test, rebuilds those binaries, and publishes the GitHub release — a tag on a commit that would fail CI does not ship. Each target uploads both `jellysink-<target>` and `jellytui-<target>`; `install.sh` treats a missing `jellytui-*` as a warning, so it still works against releases made before 0.9.0.

Lint policy lives in the root `[workspace.lints]` table and every crate sets `lints.workspace = true`, so a local `cargo clippy` matches CI.

`core` is the only crate with a `pub` surface, and it is kept to what a binary actually names: anything neither binary uses is `pub(crate)` there too, or `dead_code` stops working in core. When you add something to `core`, start it `pub(crate)` and let the compiler tell you it has to be wider. `Session` is the one item that is `pub` without a binary naming it, because `session_for_device` returns it.

Splitting the workspace moved **68 KB (−0.7%)** off `jellysink` and put **8 KB (+0.15%)** onto `jellytui` — core carries `tracing-subscriber` for `Config::set`'s `log_level` validation, which is the whole of what the frontend pays for the shared crate. Re-measure with `ls -l target/x86_64-unknown-linux-musl/release/{jellysink,jellytui}` when a dependency moves across the boundary.

## Architecture

Design notes for the trickier subsystems live in `specs/`:

- `specs/playlist.md` — the queue/mpv window invariant, when episode data is
  fetched, and why the prepend is split into two phases. **Read this before
  touching `runtime/window.rs`, `runtime/queue/` or `runtime/playback/`** in
  `crates/jellysink`; the index arithmetic has subtle invariants that
  are easy to break. `PlaylistWindow` (`runtime/window.rs`) owns all of it —
  keep its fields private and add a method rather than reaching past them.
- `specs/tracks.md` — the Jellyfin-index ↔ mpv-track-id maps, how a hand-picked
  track is remembered as an identity and re-matched in the next episode, and the
  `TrackState::settled` baseline that makes stale property changes no-ops.
  **Read this before touching `media/track.rs`, `media/streams.rs` or
  `runtime/playback/tracks.rs`** in `crates/jellysink`; the two numbering
  systems and the two subtitle
  counter gates look interchangeable and are not.
- `specs/tui.md` — why `jellytui` is a remote-control client rather than a
  second player, why its footer polls the daemon's status socket instead of
  `GET /Sessions` (that response is megabytes and cannot be trimmed), the
  staleness rules (search generations, level depths, duration and cover keys
  tied to an item id) that keep async responses from landing on the wrong
  screen, and where the cover art comes from. **Read this before touching
  `crates/jellytui/` or `crates/core/src/jellyfin/remote.rs`**, and before
  adding anything to the once-a-second poll.
- `specs/session.md` — the daemon loop: task and channel ownership, reconnect
  backoff, keepalive, report ordering, mpv generations, the latching `Signal`,
  and the `transitioning` / `stopping` contract behind `end_file_action`.
  **Read this before touching `runtime/session.rs`, `daemon/signal.rs` or
  `report.rs`** in `crates/jellysink`, and before adding a `select!` arm or a
  spawned task to a session.

### `crates/core` — `jellysink-core`

- `config/` — `paths.rs` (`Paths`: the config dir and every file in it), `settings.rs` (`Config` — config.toml — and `Field`, the exhaustive list of user-facing keys), `mpv_args.rs` (`MpvArgs`, mpv_args.conf, re-read on every mpv spawn), `credentials.rs` (`Credentials`, cred.json, mode 0600), `server.rs` (`normalize_server_url` — bare host → `http://host:8096` — and `device_name`). `atomic_write` lives in `mod.rs`, where all three writers reach it.
- `logging.rs` — `init_tracing`; `log_level` from config, overridden by `RUST_LOG` when set; uses `Targets` (not `EnvFilter`) to keep the binary small: measured at +186 KB / +2.1% for `env-filter`, even though `self_update` already links `regex` and the crate count barely moves — the trade-off is no span-field filtering. `validate_log_level` rejects a bare word that is not a level, because `Targets` would otherwise read it as a target name and silence everything; it lives here rather than in `jellysink` so `Config::set` can reject a bad value at set time. It is why `jellytui` links `tracing-subscriber` at all — the frontend never calls `init_tracing`, which writes to stdout and would paint over the alternate screen.
- `cast.rs` — `CastEvent`: the Jellyfin remote-control commands (PlayNow/Pause/Seek/…) parsed from WebSocket messages. Parsing only. Shared because `jellysink` receives these and `jellytui` sends them.
- `jellyfin/` — server API: `auth.rs` (login, `Api` client, cached auth header, `AuthExpired`, and the `get`/`post`/`post_json`/`get_json` transport helpers), `browse.rs` (the listing endpoints — `user_views`, `items` + the `ItemQuery` builder, `seasons`, `episodes`/`episodes_all`, `next_up`, `resume`, `get_item`), `remote.rs` (driving *another* session: `session_for_device`, `play_now`, `playstate`, `general_command` — note `session.rs` is the WebSocket and `remote.rs` is remote control, they are not the same thing), `model.rs` (the typed `Item`/`UserData`/`Session`/`PlayState` DTOs the frontend renders; the playback path still works in `Value` because it forwards rather than displays), `session.rs` (WebSocket URL + message parsing), `url.rs` (stream and image URLs, and `redact_api_key`), `encode_query_value` in `mod.rs`.
- `status.rs` — `PlayerStatus` / `NowPlaying`, the wire format of `jellysink status` and of jellytui's once-a-second footer poll. Serialized by the daemon, deserialized by both clients, so it belongs to neither.
- `instance.rs` — the single-instance lock and the `stop.sock` client: `stop`, `restart` (tray update) and `status`. The listener that answers them is the daemon's.
- `ticks.rs` — Jellyfin position ticks (100 ns units) ↔ seconds, plus `format_hms` for anything that prints a position. Nothing to do with media.
- `error.rs` — `UsageError` / `usage_err`.

### `crates/jellysink` — the daemon

`src/main.rs` parses the CLI (clap derive) and dispatches.

- `cli/` — one file per group of subcommands: `auth.rs` (`login`, `logout`), `config.rs`, `control.rs` (`stop`, `status` — the one-shot commands that talk to a running daemon over `stop.sock`), `run.rs` (the daemon: the lock, the tray, mpris, signals and the restart handoff), `update.rs` (CLI `update` stops a running daemon; tray **Install update** opens a terminal with `update --from-tray` and restarts in place).
- `daemon/` — the plumbing `cli/run.rs` sets up: `instance.rs` (`listen_stop`, the server side of `stop.sock`), `signal.rs` (`Signal`, a latching `watch`-channel signal used for shutdown/restart — see `specs/session.md`), `terminal.rs` (pick a terminal emulator — `xdg-terminal-exec`, `$TERMINAL`, then a known list — and spawn a command in it), `tray.rs` (optional StatusNotifier tray icon (ksni) with a quit item; shows Install update when a newer GitHub release exists), `update.rs` (GitHub Releases self-update), `mpris.rs`.
- `jellyfin/` — the endpoints only the daemon calls, as free functions over core's `Api`: `playback.rs` (`playback_info`, `post_capabilities`, and the three session reports) and `profile.rs` (device profile that requests DirectPlay; `PlayableMediaTypes` is video only). Free functions rather than an extension trait: an inherent impl cannot cross a crate boundary, and an `async fn` in a trait yields a future without a `Send` bound, which `tokio::spawn` rejects.
- `media/` — turns a Jellyfin item into a prepared mpv play: `prepare.rs` (`PreparedPlay`, `PlayRequest`, `prepare_play`, `select_media_source`), `streams.rs` (the `PlaybackInfo`/`MediaSource`/`MediaStream` serde models, the Jellyfin-index ↔ mpv-track-id maps, and the `SubtitleId`/`AudioId` identity lists), `track.rs` (the shared matcher: remembering the last hand-picked track and re-finding it in the next episode by language and track name — stream indexes are per-file, so the index alone means nothing across episodes; the remembered choice lives in `Runtime`'s `audio`/`subtitle` `TrackState` fields, in memory only and never persisted; a track picked in the mpv window counts as hand-picked too — `mpv/` observes `sid`/`aid` and `runtime::playback::adopt_mpv_track` maps those back to Jellyfin stream indexes), `subtitle.rs` and `audio.rs` (the two thin sides of it — names and log wording only), `title.rs` (display titles).
- `mpv/` — `mod.rs` owns the process and the socket: `MpvSession` spawns mpv with `--input-ipc-server`, `--force-window=yes`, `--idle=yes` (never `vo`/`hwdec`/`scale`/`glsl-shaders`, never `--no-config`), and runs the reader task. `ipc.rs` is the JSON-per-line protocol and the property coercions, which reject a null or wrong-typed answer rather than falling back to 0. `command.rs` is the typed commands (`loadfile`, `pause`, `seek_absolute`, …) and the argument shapes mpv insists on; `observe_subtitle_track` and `observe_audio_track` register the two `observe_property` calls we make. `event.rs` is `MpvEvent` and `EndFileReason`; `MpvEvent::SubtitleTrackChanged` and `MpvEvent::AudioTrackChanged` deliberately carry no value, because property changes are handled long after they are emitted and `Runtime` re-reads `sid`/`aid` instead (`TrackState::settled` is what separates a user's pick from mpv's own auto-selection). `integration_test.rs` is the real-mpv suite, see below.
- `runtime/` — the daemon loop. `mod.rs` is reconnect/backoff: one `Runtime` is built in `run` and outlives every WebSocket session, so a reconnect is non-destructive — mpv keeps playing, the queue, volume and remembered tracks stay put, and only the socket, its reader and the keepalive are per-session (`run_session` re-announces the current play once reconnected). `state.rs` is `Runtime` itself. `playback/` applies `CastEvent`s to mpv: `mod.rs` (start, stop, pause, volume), `tracks.rs` (the one `TrackKind`-parameterised path — `apply_track`, `adopt_mpv_track`, `remember_track`, `settle_track` — rather than two copies of it), `progress.rs` (sampling mpv and reporting), `load.rs` (the stream URL, whether the token rides in the Authorization header or the query string, and spawning mpv). `queue/` is series autoplay: `mod.rs` (what plays next), `expand.rs` (growing the queue from the series), `stubs.rs` (the unloaded playlist rows mpv holds either side of the playing item). `window.rs` is the `PlaylistWindow` invariant (queue + how much of it mpv holds) plus the mpv playlist semantics that go with it — `Queue`, `end_file_action`, `playlist_eof`, `queue_index_at`, `ignore_stop_for_playlist`.
- `report.rs` — reports playback state back to the Jellyfin session.

### `crates/jellytui` — the terminal frontend

A *Jellyfin remote-control client*, not a second player: it never builds a `Runtime`, never spawns mpv and never takes `instance.lock`, so it runs happily alongside the daemon. Enter on a row becomes `POST /Sessions/{id}/Playing`, which reaches the daemon over its existing WebSocket and lands in `cast.rs` exactly as a cast from the web app does — which is why adding it needed no change to the daemon at all.

- `app/` — state and the loop: `mod.rs` (`App`, the `select!` loop, key handling), `msg.rs` (what the spawned request tasks send back; one task per request so HTTP never blocks a keystroke), `browse.rs` (where the cursor is, and the browse stack), `request.rs` (every outbound browse call), `player.rs` (the once-a-second status poll and the remote-control commands a keypress turns into).
- `view/` — drawing: `mod.rs` (terminal setup, the shared layout, and the chrome — header, footer, hints — plus the panic hook that restores the terminal), `body.rs` (the three screen bodies), `grid.rs` (the tile wall), `rail.rs` (the detail rail beside a list), `playing.rs` (the `3 Playing` screen).
- `nav.rs` — the browse stack, plus `is_grid` — which view a level's rows get.
- `keys.rs` — key → `Intent`, a pure mapping so bindings are testable; `Left`/`Right` stay unresolved here because what they mean depends on the focused view.
- `cover.rs` — the `Picker`, the bounded cover cache, and the shared cell-vs-pixel geometry. Not under `view/`: it fetches and caches, it does not draw.

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
- `crates/jellysink/src/mpv/integration_test.rs` (`mpv::integration_tests`) is the exception that answers for mpv rather than for us: it drives a real player, and is where mpv behaviour we depend on but cannot fake belongs — `#EXTINF` titles surviving a `loadlist`, `insert-at` not moving the playing entry, `sid`/`aid` observers firing, and which `end-file` reason each way of ending a file produces. It plays `crates/jellysink/tests/fixtures/sample.mkv` (3 s, two audio and two subtitle tracks, 31 KB, regenerate with the `make-fixtures.sh` beside it). Add a case here when a bug turns out to be mpv doing something other than what we assumed.
- Keep the DirectPlay/no-transcode and user-mpv-config guarantees (see README "What it will not do") — they are the product's core promises.
- Anything jellytui sends must be something `cast.rs` already parses. The two halves are wired through the Jellyfin server, so a typo in a command name fails silently at runtime; `core`'s `jellyfin/remote_test.rs` round-trips every command through `CastEvent::from_ws` — which is why `cast.rs` and `jellyfin/session.rs` stay in `core` even though only the daemon connects the socket to catch that at compile-and-test time instead.
- The release carries several assets whose names all contain the target triple (`jellysink-*`, `jellytui-*`, `*.sha256`). `self_update`'s default asset selection is a substring match that would take whichever GitHub lists first, so `daemon/update.rs` pins the choice with an `asset_matcher` on the exact name. Do not remove it while more than one asset per target exists. The consequence is deliberate and documented in `specs/tui.md`: the self-updater is daemon-only, so `jellysink update` leaves `jellytui` behind and the two are expected to work version-skewed.
