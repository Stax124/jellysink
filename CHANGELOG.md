# Changelog

## 2.1.0
### Changed
- jellytui shows a single panel with a spinner while a library, series or season loads, instead of an empty list beside an empty details pane.
### Fixed
- jellytui no longer shows "loading" forever when a library, series or season fails to load: it says "could not load", and `r` retries.
- jellytui gives up on a listing after 10 seconds instead of waiting forever on an unresponsive server.

## 2.0.2
### Fixed
- jellytui Home covers no longer change shape with the item type: Continue Watching and Next Up are always landscape, and movies show their thumb or backdrop art instead of a squeezed poster.

## 2.0.1
### Fixed
- Lower idle memory use: switched from jemalloc to the musl allocator, slimmed the tray icon, and cut the tokio worker thread stack size.

## 2.0.0
### Changed
- **Breaking:** the daemon and jellytui now talk over a push-based `control.sock`, replacing `stop.sock` and jellytui's status polling. `jellysink stop` is gone tray, MPRIS, `systemctl --user stop jellysink`, or a signal.
### Added
- jellytui shows a spinner and the chosen title while playback is starting.

## 1.6.0
### Added
- jellytui renders item descriptions on more pages.
### Fixed
- jellytui previews breaking at narrow terminal widths.
- jellytui Enter doing something other than what the UI showed.
- jellytui not restoring the previous screen when going back.

## 1.5.0
### Added
- jellytui shows episodes as a cover grid.

## 1.4.0
- Internal cleanup only; no user-facing changes.

## 1.3.0
### Changed
- Smaller gap between tiles in the jellytui grid.

## 1.2.0
### Changed
- `jellysink update` and `jellytui update` restart a running daemon onto the new binary instead of stopping it; every update route updates both binaries.
### Added
- `update --force` reinstalls the latest release.

## 1.1.3
### Added
- `install.sh` adds application menu entries for jellysink and jellytui (`--no-desktop` to skip).

## 1.1.2
- Dependency updates.

## 1.1.1
### Added
- jellytui self-update: `jellytui update`, plus an update badge in the header and `u` to install. Every update route brings both binaries to the same version.

## 1.1.0
### Added
- jellytui: `t` toggles an item's watched state.
### Fixed
- MPRIS seek bar position.

## 1.0.0
### Added
- `jellytui`, a terminal frontend that remote-controls the daemon: browse, search and play from Continue Watching, Next Up and the full library, with cover art (WebP, disk cache sized by `cover_cache_mb`), rating and unplayed badges, and a log pane.
### Fixed
- Simpler, more predictable server URL parsing.

## 0.8.0
### Added
- `jellysink status` shows what a running instance is doing.
- MPRIS player: media keys, desktop now-playing widgets, `playerctl`.
### Fixed
- PlayNext desynced the queue and the mpv playlist when later episodes were already loaded.
- Building no longer requires the musl toolchain.

## 0.7.1
### Fixed
- Release pipeline; 0.7.0 was never published as a GitHub release.

## 0.7.0
### Fixed
- A WebSocket reconnect no longer interrupts playback or loses the queue, volume and remembered tracks.

## 0.6.0
### Added
- Audio track memory: the audio track you pick (in a Jellyfin client or with `#` in mpv) carries over to the next episode.

## 0.5.1
### Fixed
- Self-update no longer reports a newer minor or major version as incompatible.

## 0.5.0
### Added
- Subtitle track memory across episodes, including tracks picked in the mpv window with `j`.
### Changed
- Source builds target musl only (`cargo install` needs `--target x86_64-unknown-linux-musl`).

## 0.4.0
### Added
- Episodes that aired before the current one are prepended to mpv's playlist (`prepend_previous`, on by default).
- MIT license.

## 0.3.0
### Added
- Installing an update from the tray shows its progress in a terminal window.

## 0.2.0
### Added
- `install.sh` one-line installer.
- Tray: right-click opens the menu, and a badge marks an available update.

## 0.1.0
Initial release.
- Registers as a Jellyfin remote player and DirectPlays casts in your installed mpv, using your own mpv config.
- Series autoplay in aired order, with the remaining episodes in mpv's playlist.
- Pause, seek, volume, audio and subtitles controlled from the Jellyfin app; progress reported back to Jellyfin.
- Optional tray icon, self-update from GitHub Releases, and a systemd user unit.
