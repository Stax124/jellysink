# The daemon session

How the `run` daemon is wired: what owns what, what survives a reconnect, and
the ordering rules that keep concurrent events from producing the wrong
playback state. Almost every rule here exists because breaking it produced a
specific misbehaviour — a dropped Quit, a skipped episode, a spinning event
loop.

## The four layers

| Layer          | Lives in                    | Lifetime                                                                        |
| -------------- | --------------------------- | ------------------------------------------------------------------------------- |
| `cmd_run`      | `cli/run.rs`                | The process. Owns the lock, tray, signals.                                       |
| `runtime::run` | `runtime/session.rs`        | The process. Owns the reconnect loop, the mpv-event channel and the report sink. |
| `run_session`  | `runtime/session.rs`        | One WebSocket connection over the shared `Runtime`.                              |
| `Runtime`      | `runtime/state.rs`          | The whole daemon session. Owns the queue, mpv and the track memories.            |

**`Runtime` is built once, in `run`, before the reconnect loop starts, and the
same value is threaded into every `run_session` call as `&mut rt`.** A dropped
WebSocket does not rebuild it: mpv keeps playing, the queue and the track
memories are untouched, and `run_session`'s exit is just an `Err` the loop
reconnects from. Only the socket, its reader and the two
`tokio::time::interval`s are per-connection.

The process runs on `#[tokio::main(flavor = "current_thread")]`: one thread,
everything cooperatively scheduled.

## Tasks and channels

| Task              | Produces                             | Spawned by                       | Lifetime                            |
| ----------------- | ------------------------------------ | -------------------------------- | ----------------------------------- |
| Report sink       | nothing; consumes `report_tx`        | `run`                            | The daemon session.                 |
| mpv forwarder     | `mpv_rx` (`(generation, MpvEvent)`)  | `Runtime` (`spawn_and_load`)     | One mpv process; respawned with it. |
| WebSocket reader  | `ws_rx` (`WsIncoming`)               | `run_session`                    | One WebSocket connection.           |
| Update check      | nothing; badges the tray             | `cmd_run` (`spawn_update_check`) | Detached; ends after one check.     |
| Tray update apply | nothing; consumes `apply`            | `cmd_run`                        | Detached; the process.              |
| MPRIS emitter     | `PropertiesChanged`/`Seeked` from `status_rx` | `mpris::start`          | Detached; the D-Bus connection.     |

Two channels cross layers: `status_tx`/`status_rx` (a `watch` of `PlayerStatus`,
written by `Runtime`, read by `instance::listen_stop` for `jellysink status` and
by MPRIS) and `ext_tx`/`ext_rx` (unbounded `CastEvent`, written by MPRIS, read
by `run_session`).

Only the WebSocket reader is session-scoped. The report sink and the mpv channel
are created once in `run`, before the reconnect loop, precisely so a reconnect
does not have to re-plumb them.

**Every task spawned inside `runtime` is wrapped in an `AbortOnDrop`**
(`runtime/task.rs`) — there is no collecting struct, each call site owns its own
handle. Without it a reconnect spawns a fresh WebSocket reader and leaves the
previous one running; against a half-open TCP connection that never returns, so
it leaks for the life of the process. The three `cmd_run` and `mpris` tasks are
deliberately detached instead: they are process-scoped, and the process ends by
`exec` or exit.

`run_session`'s `select!` arms, in the order they appear:

| Arm                | On firing                                                                                                                                         |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `shutdown.fired()` | Return `Ok(())` — ends `run_session`, and the outer loop breaks.                                                                                   |
| `keepalive.tick()` | Send `{"MessageType":"KeepAlive"}`; a send failure ends the session.                                                                               |
| `progress.tick()`  | `tick_progress` — sample mpv and report, once a second.                                                                                            |
| `ws_rx.recv()`     | Dispatch the parsed `WsIncoming`; see below.                                                                                                      |
| `mpv_rx.recv()`    | `on_mpv_event`, but only for the current generation.                                                                                              |
| `ext_rx.recv()`    | An MPRIS `CastEvent` to `Runtime::handle`. Disabled by `ext_closed` once the senders are gone — MPRIS is optional, so a `None` here is not fatal.  |

(The outer `run` loop has a second, two-arm `select!`: `shutdown.fired()` or the
backoff `sleep`.)

`ws_rx` carries a parsed `WsIncoming`, not the raw frame — the reader task does
that parsing so `run_session`'s loop never blocks on it. `Cast(CastEvent)` goes
to `Runtime::handle`; `ForceKeepAlive` rebuilds the keepalive interval;
`KeepAlive` and `Ignored` are no-ops. **A `None` from `ws_rx.recv()` must return
an `Err`.** The reader owns the sender, so `None` means it is gone — the same
condition a closed socket reports. A closed receiver is ready immediately and
forever, so an empty arm body would leave this arm permanently hot and spin the
loop.

## Reconnecting

```
backoff = reconnect_delay(backoff, session_lasted, auth_expired)
sleep(backoff)
backoff = (backoff * 2).min(BACKOFF_MAX)
```

| Case                                            | Delay                                               |
| ----------------------------------------------- | --------------------------------------------------- |
| Token expired                                   | `BACKOFF_MAX` (60 s), however long the session ran.  |
| Session lasted ≥ `SESSION_HEALTHY_AFTER` (60 s) | `BACKOFF_MIN` (1 s) — it worked, this was a blip.    |
| Anything else                                   | Keep the current value.                             |

The healthy-session reset is not cosmetic: without it `backoff` only ever grows,
so a few failures at startup pin it at `BACKOFF_MAX` for the rest of the process
and a session that ran for hours waits a full minute to come back.

**Nothing in this loop touches `rt` between sessions.** Playback rides out the
gap, and mpv events raised while the socket is down stay queued on `mpv_rx` for
the next `run_session`. Once reconnected, `Runtime::reannounce` resamples mpv
and sends a fresh Start, since a server that dropped the session needs one to
show a now-playing again.

### Recognising an expired token

A 401 from any endpoint becomes `AuthExpired`, a typed error, in the single
`Api::send` wrapper. The reconnect loop checks it with `is_auth_expired`, which
walks `err.chain()` — `Report::downcast_ref` only inspects the outermost error,
so a caller adding `wrap_err` context would hide it. Matching a formatted error
chain for `"401"` instead also fires for a server on port 401 or an item id
containing `401`, which is why the type exists.

The daemon does not exit on an expired token; it logs once and retries every
60 s until `jellysink login` is run again.

## Keepalive

A 30 s interval by default. Jellyfin may send `ForceKeepAlive` carrying a
timeout in seconds; the reader converts it to `(seconds / 2).max(1)` and the
arm rebuilds the interval on the spot. A malformed or absent `Data` defaults to
60 s. Failing to *send* a keepalive ends the session, which is what puts it back
through the backoff path.

`progress` and `keepalive` use opposite missed-tick policies: `progress` skips
(`MissedTickBehavior::Skip`), `keepalive` delays (`Delay`). A progress tick that
was late is worthless; a keepalive that was late still has to be sent.

## Reporting back

`spawn_report_sink` serialises every `Report` onto one task, in FIFO order. That
is the only thing keeping a `Stopped` from overtaking the `Start` that preceded
it — `start_current` sends Stopped for the outgoing item and Start for the
incoming one microseconds apart, and Jellyfin applies them in arrival order.
Because the task is created once in `run` rather than per session, the ordering
guarantee holds across a reconnect too.

`Runtime::snapshot` builds the payload; `now_playing_queue` is an `Arc` shared
from `PlaylistWindow` rather than rebuilt per report, because a progress report
goes out once a second and carries the whole queue.

## mpv generations

The mpv-event channel is created once in `run`, but mpv itself is spawned and
killed repeatedly — once per `Stop` / next `PlayNow`, not once per WebSocket
session. Aborting the previous forwarder task (`Runtime::mpv_events`) stops it
leaking but is **not** enough on its own: events it already put on the shared
channel are still queued behind the abort.

So every event is tagged with `mpv_gen`, bumped by `spawn_and_load` and by
`stop_playback`, and the main loop drops anything that does not match the
current generation. Without it a stale `end-file` from the mpv that was just
replaced advances the queue past the episode now playing.

## Signals

`Signal` (`daemon/signal.rs`) is a latching `watch` channel, not
`Notify::notify_waiters`. `notify_waiters` stores no permit — it only wakes
futures already registered — and every receiver here re-creates its future on
each loop iteration, around loop bodies that routinely await mpv IPC with a 10 s
timeout, so a tray Quit or a `jellysink stop` landing in that window was
silently dropped. `fired()` is cancel-safe: dropping the future never consumes
the latch.

| Signal     | Fired by                                                                                             | Consumed by                                    |
| ---------- | ---------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| `shutdown` | tray Quit, MPRIS `Quit`, `stop.sock` `stop`, `cmd_run` unconditionally after its top-level `select!`  | `run_session`'s loop, `instance::listen_stop`.  |
| `restart`  | `stop.sock` `restart` (any update path)                                                              | `cmd_run`, which then `exec`s the new binary.   |
| `apply`    | tray **Install update**                                                                              | The update task in `cmd_run`.                   |

`apply` is the one edge-triggered signal, so its consumer calls `take()` to
clear the latch — otherwise the next click spins on a permanently-set latch
instead of running again.

`cmd_run` selects over the session future, the stop-socket listener, SIGTERM,
SIGINT and `restart`; whichever arm resolves, `shutdown.fire()` runs
unconditionally right after, so any way the daemon leaves that `select!` still
tells the session loop to stop. The `restart` arm additionally sets a flag and
**awaits the session future** before returning, so mpv is torn down and the
final report sent before the process replaces itself.

## Playback-lifecycle flags

Two booleans on `Runtime` gate every mpv `end-file`, and both have the same
failure mode when left set.

| Flag            | Set by                                                                       | Cleared by                                                                 |
| --------------- | ---------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `transitioning` | `start_current` (reuse), `spawn_and_load`, `advance_in_mpv`, `play_previous`  | `on_file_loaded`, `stop_playback`, and every failed step that set it.       |
| `stopping`      | `stop_playback`                                                              | `start_current`, end of `stop_playback`.                                    |

`end_file_action` (`runtime/window.rs`):

| Condition                        | Action                              |
| -------------------------------- | ----------------------------------- |
| `transitioning` or `stopping`    | `Ignore` — we caused this end-file. |
| Reason `eof` or `redirect`       | `Advance`                           |
| Reason `quit`, `stop` or `error` | `Stop`                              |
| Reason `other`                   | `Ignore`                            |

**Every step that sets `transitioning` must clear it if it fails.** Nothing
emits `file-loaded` after a failed `playlist-next` or `playlist-prev`, so the
flag stays set and `end_file_action` `Ignore`s every later end-file: autoplay
dead until the daemon restarts.

`EndFileReason` is parsed once in `mpv/event.rs` rather than carried up as a
`String` and matched in three places, so `end_file_action` matches exhaustively
and a typo cannot fall silently into the ignore arm.

### `stop` is not always a stop

`playlist-next` and an OSC jump both end the old file with reason `stop`.
`ignore_stop_for_playlist` tells them from a user Stop by whether mpv still has
a playlist (`playlist_count > 1`); if it does, the runtime waits for the
`file-loaded` that follows. What happens after that is the playlist window's
business — `specs/playlist.md`.

### Reading mpv is not optional

`playlist_state` returns an error rather than a fabricated `(0, 0)`, and
`play_next_or_stop` stops playback when it cannot read. `playlist_eof` decides
autoplay from those two numbers, so guessing puts the wrong episode on screen;
mpv failing to answer means it is gone or wedged. The same rule is why
`as_i64_property` and friends (`mpv/ipc.rs`) reject a wrong-typed answer instead
of falling back to a plausible value — `playlist-pos` → 0 and `volume` → 100
turn a transient IPC hiccup into the wrong episode.

## What survives what

| Across…                 | `Runtime` (queue, mpv, track memories)             | Instance lock |
| ----------------------- | -------------------------------------------------- | ------------- |
| A WebSocket reconnect   | **yes — the same `Runtime`, untouched**            | held          |
| `stop_playback`         | yes; queue cleared, mpv killed                     | held          |
| Update restart (`exec`) | no — `run` returns and the process replaces itself | re-acquired   |

A WebSocket drop is by design invisible to the player. Only an explicit Stop, a
daemon shutdown, or mpv exiting on its own (`MpvEvent::Exited`) tears mpv down.

## Shutdown

1. `shutdown` latches.
2. `run_session`'s first `select!` arm to see it returns `Ok(())`; the outer
   loop treats that as "done" and breaks instead of reconnecting.
3. `run` calls `rt.stop_playback(true)`: reports Stopped, `quit_and_wait`s mpv,
   clears the queue and the per-mpv-session track state (not the remembered
   tracks — see `specs/tracks.md`).
4. `quit_and_wait` escalates — IPC `quit`, 3 s, `SIGTERM`, 2 s, `SIGKILL` — then
   removes the IPC socket.
5. `instance::listen_stop` breaks its loop; `cmd_run` unlinks `stop.sock` once,
   after its `select!`, which is the only path every exit passes through — the
   restart arm never lets the listener see `shutdown`.
6. The process exits; the kernel releases the `flock` on `instance.lock`.

Step 6 is why `instance::is_running` probes the lock rather than the socket
file: a `SIGKILL` leaves `stop.sock` behind, so an `exists()` check stays true
forever. A client connecting to that leftover is refused, which it reports as
"jellysink is not running" rather than as a failure to connect.

## The restart handoff

A restart unbinds the socket for as long as the old image takes to escalate mpv
down (step 4, up to 5 s) plus the exec and the new image's startup, so a client
in that window finds a path that refuses connections — indistinguishable, on its
own, from the `SIGKILL` leftover above.

`restart.pending` in the config directory is what tells them apart. `cmd_run`
writes it in the `restart` arm, before anything else, and `bind_stop_socket`
removes it in the new image; between those two points a client retries instead
of reporting the daemon gone, up to `RESTART_HANDOFF_WAIT`. A client that waits
that out and still finds nothing removes the marker itself, so a daemon killed
mid-restart costs one wait rather than poisoning every later call.

What is retried is the whole exchange, not the connect: a restart also resets
connections it accepted on the way down, so a `status` can reach a listener that
is gone before it answers. `request` therefore treats an empty reply as a
failure worth retrying rather than as a status to parse.

This is why `bind_stop_socket` is split from `listen_stop` and called directly
after the instance lock: binding is what ends the window, and it must not sit
behind the tray or mpris's 3 s timeout.

## Known limits

- **The final `Stopped` is best-effort.** The report task is only aborted when
  `run` returns, which happens immediately after `stop_playback` queues that last
  report — so it can be cancelled before Jellyfin receives it. Jellyfin reaps the
  session on its own timeout regardless.
- **Reports are queued unboundedly.** A server that stops answering while
  playback continues accumulates one progress report per second.
- **`stop` and `restart` are not acknowledged.** The daemon answers them by
  acting, so a client cannot tell a command that was read from one accepted and
  dropped as the listener went away. One sent during the handoff window can be
  lost; `status`, which reads a reply, is retried.
- **An expired token retries forever.** The daemon stays up and polls every 60 s
  rather than exiting, so a systemd unit does not flap; the cost is a warning
  line per minute.
- **One thread.** `current_thread` flavour: a blocking call anywhere stalls
  keepalives, progress reports and mpv IPC together.
- **A wedged mpv costs 10 s per command.** That is the IPC timeout, and several
  paths issue commands in sequence.
- **`ForceKeepAlive` is trusted.** Whatever the server sends becomes the
  interval, floored at 1 s.

## Where this is tested

Backoff in `runtime/session_test.rs`; the latch in `daemon/signal_test.rs`; the
end-file gating in `runtime/window_test.rs`; `AuthExpired` surviving added
context in `crates/core/src/jellyfin/auth_test.rs`; the IPC coercions and
end-file reasons in `mpv/ipc_test.rs` and `mpv/event_test.rs`.
