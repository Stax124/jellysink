# jellytui

`jellytui` browses Jellyfin in the terminal and starts playback in a running
jellysink. This note covers the decisions the code cannot state for itself:
why it is a client of the daemon rather than a second copy of it, and why its
per-second state comes from a Unix socket rather than the obvious HTTP call.

## It is a remote, not a player

The daemon already registers as a controllable Jellyfin session
(`Api::post_capabilities` → `POST /Sessions/Capabilities/Full`, at the top of
every `run_session`), and `src/cast.rs` already parses the whole remote-control
vocabulary. So the frontend does not need any of the playback machinery — it
sends the same commands the web app sends, and the server relays them over the
WebSocket the daemon is already holding:

```
jellytui ──HTTP──> Jellyfin ──WebSocket──> jellysink ──> mpv
```

Every action maps onto something `cast.rs` already handles:

| jellytui | HTTP | arrives as |
| --- | --- | --- |
| Enter on a row | `POST /Sessions/{id}/Playing?PlayCommand=PlayNow&ItemIds=…` | `CastEvent::PlayNow` |
| space, `s`, `n`, `p`, seek | `POST /Sessions/{id}/Playing/{command}` | `CastEvent::PlayPause`, `Stop`, `Next`, … |
| volume, mute, fullscreen | `POST /Sessions/{id}/Command` | `CastEvent::SetVolume`, … |

Because of this, adding the frontend changed nothing in `runtime/`, `mpv/`,
`report.rs` or `cast.rs`. Queue building, series autoplay, remembered tracks and
progress reporting are the daemon's, unchanged.

The alternative — a `jellysink browse` subcommand owning its own `Runtime` —
was rejected: `InstanceLock::acquire` is exclusive and `paths.mpv_socket()` is a
fixed path, so it could not run beside the daemon most users have under systemd,
and it would have meant two ways to be a player.

**Consequence to keep in mind:** the frontend is useless without a running
daemon, and it says so rather than failing obscurely (`session_id` sets the
"jellysink is not connected" line). Anything it sends must be something
`cast.rs` parses; the two halves are joined through a third process, so a
misspelled command name fails silently at runtime. `jellyfin/remote_test.rs`
round-trips every command through `CastEvent::from_ws` to catch that in CI.

## Finding the daemon's session

`GET /Sessions?deviceId={device_id}`, using the device id from `cred.json` —
*not* a match on the client name. A reinstall leaves a stale session behind with
the same `Client` of `"jellysink"`, and commands posted to that one are
delivered nowhere. Sharing the stored credentials means the server sees one
session for both processes, which is what makes "my target is my own session"
true; the cost is that jellytui only drives a jellysink on the same machine.

## Updating

`jellysink update` and the tray's Install update replace the **daemon only**:
`app/update.rs` pins the release asset to `jellysink-<target>` by exact name,
because every asset of a release carries the target triple in its name and
`self_update`'s default substring match would otherwise be free to pick
`jellytui-<target>` or a `.sha256`. `install.sh` installs and updates both.

So a self-updated machine can run a newer daemon beside an older `jellytui`.
That is expected to keep working: the two are joined only by the command names
in `cast.rs`, which are Jellyfin's own and do not change. If that ever stops
being true, the frontend needs a version check — not a shared updater.

## Staying responsive

Every request runs in a spawned task that reports back over one `mpsc` channel
(`Msg`), so no keystroke ever waits on HTTP. Two orderings matter:

- **Search** is debounced 250 ms and each request carries a generation number.
  A slow earlier response is dropped rather than overwriting a newer one.
- **Level loads** carry their depth. If the user goes back before the rows
  arrive, the depth no longer indexes into the stack and the rows are dropped.

`PlaylistWindow`-style index care applies to `Level::fill` too: a reload that
returns fewer rows pulls the cursor back into range, because the selection
outlives the list it points into.

## Where the footer's state comes from

Not from `GET /Sessions`. That response embeds `NowPlayingQueueFullItems` — a
full item DTO for every entry in the play queue — and **no request parameter
trims it**: `fields`, `EnableFullItems` and `enableImages` were all measured
against a real server and changed nothing. With a 103-episode series queued it
is **2.4 MB**, and it stays that large after playback stops because the queue
persists. Polled once a second on a `current_thread` runtime, that is megabytes
of download, TLS decryption and JSON parsing per second on the same thread that
draws the UI — which is exactly what it feels like.

So the footer polls the daemon's own status socket instead
(`instance::request_status`, the one `jellysink status` uses): **79 bytes** over
a Unix socket, no TLS, no JSON tree. It also carries the queue position, which
`/Sessions` made us compute. The blocking call runs in `spawn_blocking` so a
wedged daemon cannot stall the loop.

`/Sessions` is still needed for the session id that addresses commands, but that
is fetched **once** in the background at startup and cached, not polled.

Two things follow:

- The status socket carries no duration, so the total for the progress bar is
  fetched with one `/Items/{id}` request per item change and cached against that
  item id — a stale total must never label a new episode.
- The footer's title is the daemon's `display_title`, not `Item::label`, so it
  reads the same as the mpv window title regardless of who started playback.

This also means the footer works for playback started from a phone or the web
app, not only from jellytui.

Seeking is computed from the last polled position, so it can be up to a second
stale — invisible at ten-second steps.

## Terminal ownership

mpv is a separate process with its own window and all three stdio handles on
`/dev/null`, so it never contends for the terminal.

Two rules follow from owning the alternate screen:

- `jellytui` never calls `init_tracing`. It is a `fmt` subscriber on stdout and
  would paint over the UI. With no subscriber the `tracing` macros are no-ops.
- `ui::enter` installs a panic hook that restores the terminal before
  delegating, so a panic (or a color_eyre report) does not leave the user in a
  raw-mode alternate screen.

`crossterm::event::read` blocks, so input is read on a dedicated OS thread and
forwarded over a channel into the `select!` loop. That thread is detached: at
exit it is parked in `read`, and the process is going away anyway.
