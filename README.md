<div align="center">
  <img src="assets/logo.svg" alt="jellysink logo" width="128" height="128">

  <h1>jellysink</h1>

  <p>Efficient MPV cast target for Jellyfin on Linux</p>

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

Configuration is CLI-only. You will probably only use it once, to log in.

## Motivation

I really liked using the [jellyfin-mpv-shim](https://github.com/jellyfin/jellyfin-mpv-shim) project, but it is unfortunately written in Python and likes to eat a lot of RAM. I needed something that is fast and lightweight, so that it can run as a background service.

This project is not trying to be a full replacement for jellyfin-mpv-shim. It is a minimal implementation of its core functionality that I care about in my daily life.

| |jellysink  |  jellyfin-mpv-shim |
|---|------------------|-----------|
| Language | Rust | Python |
| Idle RAM usage | ~10 MB | ~250 MB |
| DirectPlay / DirectStream | ✅ | ✅ |
| Series autoplay | ✅ | ✅ |
| Pause, seek, volume, audio, and subtitles | ✅ | ✅ |
| Progress reporting back to Jellyfin | ✅ | ✅ |
| Next episode list in MPV playlist | ✅ | ❌ |
| Respects your MPV configuration | ✅ | ❌ |
| GUI for configuration | ❌ | ✅ |
| Quick Connect | ❌ | ✅ |
| Transcoding | ❌ | ✅ |
| SyncPlay | ❌ | ✅ |
| Multiple simultaneous streams | ❌ | ✅ |

## Features

- Appears as a remote player in the Jellyfin web, Android, and iOS apps
- DirectPlay / DirectStream only — the original stream reaches mpv
- Uses your installed mpv and your existing config; never sets `vo`, `hwdec`, `scale`, or `glsl-shaders`, and never passes `--no-config`
- Series autoplay in aired order, across seasons, until the last episode or Stop
- Remaining episodes are appended to mpv’s playlist (`<` / `>` or the OSC playlist)
- Optional StatusNotifier tray icon (KDE, GNOME AppIndicator, Waybar, …)
- Self-update from GitHub Releases (`jellysink update`, or **Install update** in the tray)

## Installation

Linux (x86_64 or aarch64). Requires [`mpv`](https://mpv.io/).

```sh
curl -fsSL https://raw.githubusercontent.com/Stax124/jellysink/main/install.sh | sh
```

This installs a musl binary to `~/.local/bin/jellysink` and a user systemd unit. Then:

```sh
jellysink login
systemctl --user enable --now jellysink
```

Skip the unit with `curl -fsSL ... | sh -s -- --no-systemd`, then run `jellysink run` yourself.

`jellysink` checks GitHub Releases once when the daemon starts. If a newer version exists, the tray gets an **Install update** item; choosing it replaces the binary and restarts the daemon. `jellysink update` installs from the CLI and **stops** a running instance — start it again with `systemctl --user start jellysink` or `jellysink run`. Current playback ends either way.

### From source

```bash
git clone https://github.com/Stax124/jellysink.git
cd jellysink
cargo build --release
install -Dm755 target/release/jellysink ~/.local/bin/jellysink
install -Dm644 systemd/jellysink.service ~/.config/systemd/user/jellysink.service
```

Or, without cloning:

```bash
cargo install --git https://github.com/Stax124/jellysink
```

A locally built gnu binary still updates from the musl GitHub asset for the same architecture.

## Usage

```bash
jellysink login          # server URL, username, password
jellysink run            # default if you pass no subcommand
jellysink stop           # ask a running instance to quit
jellysink update         # install the latest GitHub release
jellysink update --check # print whether a newer release exists
```

Cast a movie or episode to **jellysink** from the Jellyfin web/Android/iOS app. mpv opens with your normal config. Pause, seek, volume, mute, fullscreen, audio, and subtitles work from the controlling app. A series episode continues into the next one (aired order, across seasons) until the last episode or Stop.

Quit with the tray icon, `jellysink stop`, SIGTERM, or SIGINT.

```bash
jellysink config path
jellysink config get
jellysink config set mpv_path /usr/bin/mpv
jellysink config set log_level debug
jellysink config set autoplay false
jellysink logout
```

Bare host names become `http://host:8096`. Write `:80` if you really want port 80.

## Configuration

| Key         | Default | Notes |
|-------------|---------|--------|
| `mpv_path`  | `mpv`   | Binary used to spawn the player |
| `log_level` | `info`  | `tracing` filter (`error`, `warn`, `info`, `debug`, `trace`); `RUST_LOG` overrides this if set |
| `autoplay`  | `true`  | Next episode in aired order; `false` stops after the current item |
| `mpv_args`  | _(empty)_ | Extra argv on top of your mpv config, never instead of it |

`--config DIR` (global) uses a different configuration directory. The default is `~/.config/jellysink`.

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

### Requirements

- A [Rust](https://rustup.rs/) toolchain (edition 2024)
- [`mpv`](https://mpv.io/) on `PATH` (or set `mpv_path`)
- A [Jellyfin](https://jellyfin.org/) server
- Optional: a StatusNotifier tray host

## Acknowledgements

- [Jellyfin](https://jellyfin.org/) for the server and API
- [mpv](https://mpv.io/) for the player
- [jellyfin-mpv-shim](https://github.com/jellyfin/jellyfin-mpv-shim) for inspiration

## Contributing

Contributions are welcome! Please open an issue or a pull request on GitHub.

## License

[MIT](https://opensource.org/licenses/MIT)