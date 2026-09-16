<div align="center">
  <img src="assets/logo.svg" alt="jellysink logo" width="128" height="128">

  <h1>jellysink</h1>

  <p>Stream your favourite shows from Jellyfin into your local MPV Player</p>

  <p>
    <a href="https://github.com/Stax124/jellysink"><img src="https://img.shields.io/badge/GitHub-Stax124%2Fjellysink-181717?style=flat-square&logo=github" alt="GitHub"></a>
    <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-2024_edition-000000?style=flat-square&logo=rust" alt="Rust"></a>
    <a href="https://jellyfin.org/"><img src="https://img.shields.io/badge/Jellyfin-DirectPlay-00A4DC?style=flat-square&logo=jellyfin&logoColor=white" alt="Jellyfin"></a>
    <a href="https://mpv.io/"><img src="https://img.shields.io/badge/player-mpv-691F69?style=flat-square" alt="mpv"></a>
    <img src="https://img.shields.io/badge/platform-Linux-grey?style=flat-square&logo=linux&logoColor=white" alt="Linux">
    <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT license"></a>
    <a href="https://github.com/Stax124/jellysink/stargazers"><img src="https://img.shields.io/github/stars/Stax124/jellysink?style=flat-square" alt="GitHub stars"></a>
    <a href="https://github.com/Stax124/jellysink/issues"><img src="https://img.shields.io/github/issues/Stax124/jellysink?style=flat-square" alt="GitHub issues"></a>
  </p>
</div>

jellysink registers as a remote player, receives Play commands from the web or mobile apps, and DirectPlays the stream in your installed **mpv** — including whatever shaders and upscalers you already have in `~/.config/mpv/mpv.conf`.

It ships with **`jellytui`**, a terminal frontend, so you can browse your library and start playback without reaching for a phone or a browser.

Configuration is CLI-only. You will probably only use it once, to log in.

## Motivation

I really liked using the [jellyfin-mpv-shim](https://github.com/jellyfin/jellyfin-mpv-shim) project, but it is unfortunately written in Python and likes to eat a lot of RAM. I needed something that is fast and lightweight, so that it can run as a background service.

This project is not trying to be a full replacement for jellyfin-mpv-shim. It is a minimal implementation of its core functionality that I care about in my daily life.

|                                           | jellysink | jellyfin-mpv-shim |
| ----------------------------------------- | --------- | ----------------- |
| Language                                  | Rust      | Python            |
| Idle RAM usage                            | ~10 MB    | ~250 MB           |
| DirectPlay / DirectStream                 | ✅         | ✅                 |
| Series autoplay                           | ✅         | ✅                 |
| Pause, seek, volume, audio, and subtitles | ✅         | ✅                 |
| Progress reporting back to Jellyfin       | ✅         | ✅                 |
| MPV playlist integration                  | ✅         | ❌                 |
| Respects your MPV configuration           | ✅         | ❌                 |
| Terminal frontend (browse and play)       | ✅         | ❌                 |
| GUI for configuration                     | ❌         | ✅                 |
| Quick Connect                             | ❌         | ✅                 |
| Transcoding                               | ❌         | ✅                 |
| SyncPlay                                  | ❌         | ✅                 |
| Multiple simultaneous streams             | ❌         | ✅                 |

## Features

- Appears as a remote player in the Jellyfin web, Android, and iOS apps
- `jellytui`: browse, search and play from the terminal — Continue Watching, Next Up, and the whole library tree
- DirectPlay / DirectStream only — the original stream reaches mpv
- Uses your installed mpv and your existing config; never sets `vo`, `hwdec`, `scale`, or `glsl-shaders`, and never passes `--no-config`
- Series autoplay in aired order, across seasons, until the last episode or Stop
- Remembers the audio and subtitle tracks you pick — in a Jellyfin client or in mpv itself — and re-selects them on the next episode
- Remaining episodes are appended to mpv’s playlist (`<` / `>` or the OSC playlist)
- Optional StatusNotifier tray icon (KDE, GNOME AppIndicator, Waybar, …)
- Optional MPRIS player (`org.mpris.MediaPlayer2`) — media keys, GNOME/KDE now-playing widgets, lock-screen controls, `playerctl`
- Self-update from GitHub Releases — any of `jellysink update`, **Install update** in the tray, or `u` in `jellytui` brings both binaries level

## Installation

Linux (x86_64 or aarch64). Requires [`mpv`](https://mpv.io/) **0.38 or newer** — queueing episodes ahead of the one playing uses `loadlist insert-at`, which older players reject.

```sh
curl -fsSL https://raw.githubusercontent.com/Stax124/jellysink/main/install.sh | sh
```

This installs musl binaries to `~/.local/bin/jellysink` and `~/.local/bin/jellytui`, a user systemd unit, and application menu entries for both under `~/.local/share`. Then:

```sh
jellysink login
systemctl --user enable --now jellysink
```

Skip the unit with `curl -fsSL ... | sh -s -- --no-systemd`, then run `jellysink run` yourself. `--no-desktop` skips the menu entries.

`jellysink` checks GitHub Releases once when the daemon starts. If a newer version exists, the tray icon gets a green-dot badge and the menu gets an **Install update** item; choosing it opens a terminal, shows download progress, replaces the binary, restarts the daemon, then waits for Enter so the window stays open. `jellysink update` installs from the CLI and **stops** a running instance — start it again with `systemctl --user start jellysink` or `jellysink run`. Current playback ends either way. Either route also brings the `jellytui` sitting beside the daemon level, whether or not the daemon itself had an update.

### From source

Builds are musl-only — the same static target the releases ship — so a glibc
toolchain is rejected at build time. You need the musl target and a musl C
compiler (`musl` on Arch, `musl-tools` on Debian/Ubuntu):

```bash
rustup target add x86_64-unknown-linux-musl   # or aarch64-unknown-linux-musl
```

```bash
git clone https://github.com/Stax124/jellysink.git
cd jellysink
cargo build --release
install -Dm755 target/x86_64-unknown-linux-musl/release/jellysink ~/.local/bin/jellysink
install -Dm755 target/x86_64-unknown-linux-musl/release/jellytui ~/.local/bin/jellytui
install -Dm644 systemd/jellysink.service ~/.config/systemd/user/jellysink.service
install -Dm644 desktop/jellysink.desktop ~/.local/share/applications/jellysink.desktop
install -Dm644 desktop/jellytui.desktop ~/.local/share/applications/jellytui.desktop
install -Dm644 assets/logo.svg ~/.local/share/icons/hicolor/scalable/apps/jellysink.svg
```

On an aarch64 host, `export CARGO_BUILD_TARGET=aarch64-unknown-linux-musl` first; it
overrides the repository default, and the output path changes to match.

Or, without cloning — the repository's cargo config does not apply here, so name the
target yourself:

```bash
cargo install --git https://github.com/Stax124/jellysink --target x86_64-unknown-linux-musl
```

## Usage

```bash
jellysink login          # server URL, username, password
jellysink run            # default if you pass no subcommand
jellysink stop           # ask a running instance to quit
jellysink status         # show what a running instance is doing
jellysink update         # install the latest GitHub release
jellysink update --check # print whether a newer release exists
jellysink update --force # reinstall the latest release even if already on it

jellytui                 # browse and play from the terminal
jellytui update          # install the latest GitHub release
jellytui update --check  # print whether a newer release exists
jellytui update --force  # reinstall the latest release even if already on it
```

Cast a movie or episode to **jellysink** from the Jellyfin web/Android/iOS app. mpv opens with your normal config. Pause, seek, volume, mute, fullscreen, audio, and subtitles work from the controlling app. A series episode continues into the next one (aired order, across seasons) until the last episode or Stop, carrying the audio and subtitle tracks you last picked with it — picked in the controlling app, or with `#` and `j` in the mpv window.

When you cast an episode, the episodes that aired before it are loaded into mpv's playlist too, so the playlist selector (and Previous) can reach the whole series rather than only what follows. Set `prepend_previous` to `false` to only look for next episodes.

Quit with the tray icon, `jellysink stop`, SIGTERM, or SIGINT.

## Terminal frontend

`jellytui` is a Jellyfin client, not a second player: it asks the running jellysink to
play, exactly as the web app does. So the daemon must be running, and everything it
already does — DirectPlay, series autoplay, remembered tracks, progress reporting —
applies unchanged. It logs in with the credentials `jellysink login` already stored.

Playback control stays in mpv: `jellytui` starts something playing and shows what is
playing, but pause, seek, volume, mute and fullscreen are mpv's own keys in the mpv
window, not a second set of bindings here.

Quitting `jellytui` does not stop playback — it is only a remote.

It checks GitHub Releases once at startup. If a newer version exists the header shows
`↑<version> u`; pressing `u` leaves the terminal UI, installs, and starts the new version
straight back up. It updates the `jellysink` binary too, but never stops the daemon:
replacing the file leaves a running jellysink on the copy it already has, so playback is
undisturbed and it picks the new version up on its next restart — which it will tell you
how to do.

## Configuration commands

```bash
jellysink config path
jellysink config get
jellysink config set mpv_path /usr/bin/mpv
jellysink config set autoplay false
jellysink logout
```

## Configuration

| Key                | Default   | Notes                                                                                                 |
| ------------------ | --------- | ----------------------------------------------------------------------------------------------------- |
| `mpv_path`         | `mpv`     | Binary used to spawn the player                                                                       |
| `autoplay`         | `true`    | Next episode in aired order; `false` stops after the current item                                     |
| `prepend_previous` | `true`    | Also load the episodes that aired *before* the current one, so mpv's playlist selector can reach them |
| `cover_cache_mb`   | `256`     | Disk jellytui's cover cache may use, in `~/.cache/jellysink/covers`; `0` turns it off                 |
| `mpv_args`         | _(empty)_ | Extra argv on top of your mpv config, never instead of it                                             |

`--config DIR` (global) uses a different configuration directory. The default is `~/.config/jellysink`.

Logging is `info` unless `RUST_LOG` says otherwise (`error`, `warn`, `info`, `debug`, `trace`, or a `tracing` filter like `jellysink=debug,warn`). It sets what jellytui's `L` pane shows too.

`mpv_args` is stored in `~/.config/jellysink/mpv_args.conf` (one argument per line; `#` comments allowed) and re-read every time mpv is spawned, so a running daemon picks up changes on the next play — no restart needed. You can also edit the file directly.

```bash
jellysink config set mpv_args "--fullscreen"
```

Credentials live in `~/.config/jellysink/cred.json` (mode `0600`). The password is not stored.

## Upscaling

This program never sets `vo`, `hwdec`, `scale`, or `glsl-shaders`, and it never passes `--no-config`. Put your upscaler in mpv:

```
# ~/.config/mpv/mpv.conf
glsl-shaders="~~/shaders/Anime4K_*.glsl"
```

## What it will not do

- Transcode. If the server will not DirectPlay/DirectStream the file, playback is refused so the upscaler still sees the original stream.
- Audio-only items (music). The session advertises video only.
- Quick Connect, SyncPlay, the in-window library, Live TV UI, offline sync.

## Development

Design notes for the trickier subsystems live in [`specs/`](specs/):

- [`specs/playlist.md`](specs/playlist.md) — how the queue is built, when
  episode data is fetched, and how entries reach mpv's playlist.
- [`specs/tui.md`](specs/tui.md) — how `jellytui` drives the daemon, and why it
  is a remote-control client rather than a second player.

### Requirements

- A [Rust](https://rustup.rs/) toolchain (edition 2024)
- [`mpv`](https://mpv.io/) on `PATH` (or set `mpv_path`)
- A [Jellyfin](https://jellyfin.org/) server
- Optional: a StatusNotifier tray host, and/or a D-Bus session bus for MPRIS

## Acknowledgements

- [Jellyfin](https://jellyfin.org/) for the server and API
- [mpv](https://mpv.io/) for the player
- [jellyfin-mpv-shim](https://github.com/jellyfin/jellyfin-mpv-shim) for inspiration

## Contributing

Contributions are welcome! Please open an issue or a pull request on GitHub.

## License

[MIT](https://opensource.org/licenses/MIT)