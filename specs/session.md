# The daemon session

How the `run` daemon is wired: what owns what, what survives a reconnect, and
the ordering rules that keep concurrent events from producing the wrong
playback state.

This document describes the system as it stands, including *why* the awkward
parts are the way they are. Almost everything here is a rule that exists
because breaking it produced a specific misbehaviour — a dropped Quit, a
skipped episode, a spinning event loop — and those are recorded next to the
rule rather than left to be rediscovered.

## The four layers

| Layer                   | Lives in                 | Lifetime                                   |
| ----------------------- | ------------------------ | ------------------------------------------ |
| `cmd_run`               | `src/app/cli.rs`             | The process. Owns the lock, tray, signals. |
| `runtime::run`          | `src/runtime/session.rs` | The process. Owns the reconnect loop, the mpv-event channel and the report sink. |
| `run_session`           | `src/runtime/session.rs` | One WebSocket connection over the shared `Runtime`. |
| `Runtime`               | `src/runtime/state.rs`   | The whole daemon session. Owns the queue, mpv and the track memories. |

(`src/runtime/mod.rs` is just `pub(crate) use session::run;` plus the other
module declarations — `run` and `run_session` live in `session.rs`.)

**`Runtime` is built once, in `run`, before the reconnect loop starts, and the
same value is threaded into every `run_session` call as `&mut rt`.** A dropped
WebSocket does not rebuild it: mpv keeps playing, the queue and the track
memories are untouched, and `run_session`'s exit is just an `Err` the loop
reconnects from. Only the socket, its reader and the two `tokio::time::interval`s
are per-connection.

The process runs on `#[tokio::main(flavor = "current_thread")]`: one thread,
everything cooperatively scheduled.

## Tasks and channels

`run` spawns two long-lived tasks before the loop starts, and `run_session`
spawns one more per connection.

| Task            | Produces                        | Spawned by    | Lifetime                        |
| --------------- | -------------------------------- | ------------- | -------------------------------- |
| Report sink     | nothing; consumes `report_tx`   | `run`          | The daemon session.              |
| mpv forwarder   | `mpv_rx` (`(generation, MpvEvent)`) | `Runtime` (`spawn_and_load`) | One mpv process; respawned with it. |
| WebSocket reader| `ws_rx` (`WsIncoming`)            | `run_session`  | One WebSocket connection.        |

Only the WebSocket reader is session-scoped. The report sink and the mpv
channel are created once in `run`, before the reconnect loop, precisely so a
reconnect does not have to re-plumb them. Every spawned task is wrapped in an
`AbortOnDrop` (`src/runtime/task.rs`) so dropping the handle aborts the task —
there is no single collecting struct, each call site owns its own handle.
Without it a reconnect spawned a fresh WebSocket reader and left the previous
one running; against a half-open TCP connection that never returns, so it
leaked for the life of the process.

`run_session`'s `select!` arms, in the order they appear:

| Arm              | On firing                                                     |
| ---------------- | ---------------------------------------------------------------|
| `shutdown.fired()` | Return `Ok(())` — ends `run_session`, and the outer loop breaks. |
| `keepalive.tick()` | Send `{"MessageType":"KeepAlive"}`; a send failure ends the session. |
| `progress.tick()`| `tick_progress` — sample mpv and report, once a second.        |
| `ws_rx.recv()`   | Dispatch the parsed `WsIncoming`; see below.                    |
| `mpv_rx.recv()`  | `on_mpv_event`, but only for the current generation.            |

`ws_rx` carries a parsed `WsIncoming`, not the raw WebSocket frame — the reader
task does that parsing so `run_session`'s own loop never blocks on it. Its
variants: `Cast(CastEvent)` goes to `Runtime::handle`; `ForceKeepAlive`
rebuilds the keepalive interval (see below); `KeepAlive` and `Ignored` are
no-ops (the latter logged at `debug`). **A `None` from `ws_rx.recv()` must
return an `Err`.** The reader owns the sender, so `None` means it is gone — the
same condition a closed socket reports. A closed receiver is ready immediately
and forever, so an empty arm body here would leave this arm permanently hot and
spin the loop.

## Reconnecting

`run` loops: run a session, decide a delay, sleep, double the delay.

```
backoff = reconnect_delay(backoff, session_lasted, auth_expired)
sleep(backoff)
backoff = (backoff * 2).min(BACKOFF_MAX)
```

| Case                                     | Delay                                  |
| ----------------------------------------- | --------------------------------------- |
| Token expired                            | `BACKOFF_MAX` (60 s), however long the session ran. |
| Session lasted ≥ `SESSION_HEALTHY_AFTER` (60 s) | `BACKOFF_MIN` (1 s) — it worked, this was a blip. |
| Anything else                            | Keep the current value.                |

The healthy-session reset is not cosmetic. Without it `backoff` only ever grew:
a few failures at startup pinned it at `BACKOFF_MAX` for the rest of the
process, so a session that ran for hours and then dropped waited a full minute
to come back.

**Nothing in this loop touches `rt` between sessions.** Playback rides out the
gap, and mpv events raised while the socket is down stay queued on `mpv_rx` for
the next `run_session` to pick up. Once reconnected, `Runtime::reannounce`
resamples mpv and sends a fresh Start, since a server that dropped the session
needs one to show a now-playing again.

### Recognising an expired token

A 401 from any endpoint becomes `AuthExpired`, a typed error, in the single
`Api::send` wrapper. The reconnect loop checks it with `is_auth_expired`, which
walks `err.chain()` — `Report::downcast_ref` only inspects the outermost error,
so a caller adding `wrap_err` context would hide it. Matching a formatted error
chain for `"401"` instead also fired for a server on port 401 or an item id
containing `401`, which is why the type exists.

The daemon does not exit on an expired token; it logs once and retries every
60 s until `jellysink login` is run again.

## Keepalive

A 30 s interval by default. Jellyfin may send `ForceKeepAlive` carrying a
timeout in seconds; the reader converts it to `(seconds / 2).max(1)` and the
`WsIncoming::ForceKeepAlive` arm rebuilds the interval on the spot. A malformed
or absent `Data` defaults to 60 s.

Failing to *send* a keepalive ends the session with an error, which is what puts
it back through the backoff path.

`progress` and `keepalive` use opposite missed-tick policies: `progress` skips
missed ticks (`MissedTickBehavior::Skip`), `keepalive` delays them
(`MissedTickBehavior::Delay`). A progress tick that was late is worthless; a
keepalive that was late still has to be sent.

## Reporting back

`spawn_report_sink` serialises every `Report` onto one task, in FIFO order.
This is the only thing keeping a `Stopped` from overtaking the `Start` that
preceded it — `start_current` sends Stopped for the outgoing item and Start for
the incoming one microseconds apart, and Jellyfin applies them in arrival order.
Because the task is created once in `run` rather than per session, this
ordering guarantee holds across a reconnect too, not just within one
connection.

`Runtime::snapshot` builds the payload; `now_playing_queue` is an `Arc` shared
from `PlaylistWindow` rather than rebuilt per report, because a progress report
goes out once a second and carries the whole queue.

The channel is unbounded, and the report task is only aborted when `run`
itself returns — at full daemon shutdown, not at the end of a session. The
final `Stopped` that `run` sends after the reconnect loop breaks is still
best-effort, though: it is queued and `run` returns immediately after, so the
task can be aborted before it has actually delivered that last message — see
[Known limits](#known-limits).

## mpv generations

The mpv-event channel (`mpv_tx` / `mpv_rx`) is created once in `run`, but mpv
itself is spawned and killed repeatedly over the life of the daemon — once per
`Stop` / next `PlayNow`, not once per WebSocket session. Aborting the previous
forwarder task (`Runtime::mpv_events`) stops it leaking but is **not** enough
on its own: events it already put on the shared channel are still queued behind
the abort.

So every event is tagged with `mpv_gen`, bumped by `spawn_and_load` and by
`stop_playback`, and `run_session`'s main loop drops anything that does not
match the current generation. Without it a stale `end-file` from the mpv that
was just replaced advances the queue past the episode that is now playing.

## Signals

`Signal` (`src/app/signal.rs`) is a latching `watch` channel, not
`Notify::notify_waiters`. `notify_waiters` stores no permit — it only wakes
futures already registered — and every receiver in this crate re-creates its
future on each loop iteration, around loop bodies that routinely await mpv IPC
with a 10 s timeout. A tray Quit or a `jellysink stop` landing in that window
was silently dropped. `fired()` is cancel-safe: dropping the future never
consumes the latch.

| Signal     | Fired by                                        | Consumed by                                  |
| ---------- | ------------------------------------------------ | --------------------------------------------- |
| `shutdown` | tray Quit, `stop.sock` `stop`, `cmd_run` unconditionally after its top-level `select!` | `run_session`'s loop, `instance::listen_stop`. |
| `restart`  | `stop.sock` `restart` (tray update)             | `cmd_run`, which then `exec`s the new binary. |
| `apply`    | tray **Install update**                          | The update task in `cmd_run`.                |

`apply` is the one edge-triggered signal, so its consumer calls `take()` to
clear the latch — otherwise the next click would spin on a permanently-set
latch instead of running again.

`cmd_run` selects over the session future, the stop-socket listener, SIGTERM,
SIGINT and `restart`; whichever arm resolves, `shutdown.fire()` runs
unconditionally right after — so any way the daemon leaves that `select!`
still tells the session loop to stop. The `restart` arm additionally sets a
flag and **awaits the session future** before returning, so mpv is torn down
and the final report sent before the process replaces itself.

## Playback-lifecycle flags

Two booleans on `Runtime` gate every mpv `end-file`, and both have the same
failure mode when left set.

| Flag            | Set by                                                             | Cleared by                        |
| --------------- | -------------------------------------------------------------------| ---------------------------------- |
| `transitioning` | `start_current` (reuse), `spawn_and_load`, `advance_in_mpv`, `play_previous` | `on_file_loaded`, `stop_playback`, and every failed step that set it. |
| `stopping`      | `stop_playback`                                                    | `start_current`, end of `stop_playback`. |

`end_file_action` (`src/runtime/window.rs`):

| Condition                        | Action                                     |
| ---------------------------------| --------------------------------------------|
| `transitioning` or `stopping`    | `Ignore` — we caused this end-file.        |
| Reason `eof` or `redirect`       | `Advance`                                  |
| Reason `quit`, `stop` or `error` | `Stop`                                     |
| Reason `other`                   | `Ignore`                                   |

**Every step that sets `transitioning` must clear it if it fails.** Nothing will
emit `file-loaded` after a failed `playlist-next` or `playlist-prev`, so the
flag would stay set and `end_file_action` would `Ignore` every later end-file:
autoplay dead until the daemon restarts. `advance_in_mpv` (`src/runtime/queue.rs`)
and `play_previous` (`src/runtime/state.rs`) both do this explicitly.

`EndFileReason` is parsed once in `src/mpv/mod.rs` rather than carried up as a
`String` and matched in three places, so `end_file_action` can match
exhaustively and a typo cannot fall silently into the ignore arm.

### `stop` is not always a stop

`playlist-next` and an OSC jump both end the old file with reason `stop`.
`ignore_stop_for_playlist` distinguishes them from a user Stop by whether mpv
still has a playlist (`playlist_count > 1`); if it does, the runtime waits for
the `file-loaded` that follows. What happens after that — adopting the new
index, re-preparing, reporting — is the playlist window's business and lives in
`specs/playlist.md`.

### Reading mpv is not optional

`playlist_state` (`src/runtime/queue.rs`) returns an error rather than a
fabricated `(0, 0)`, and `play_next_or_stop` stops playback when it cannot
read. `playlist_eof` decides autoplay from those two numbers, so guessing puts
the wrong episode on screen; mpv failing to answer means it is gone or wedged.
The same rule is why `as_i64_property` and friends (`src/mpv/mod.rs`) reject a
wrong-typed answer instead of falling back to a plausible value —
`playlist-pos` → 0 and `volume` → 100 used to make a transient IPC hiccup play
the wrong episode.

## What survives what

| Across…                | `Runtime` (queue, mpv, track memories) | Instance lock |
| ----------------------- | ---------------------------------------| --------------|
| A WebSocket reconnect  | **yes — the same `Runtime`, untouched** | held          |
| `stop_playback`        | yes; queue cleared, mpv killed         | held          |
| Update restart (`exec`)| no — `run` returns and the process replaces itself | re-acquired |

A WebSocket drop is, by design, invisible to the player: mpv is not killed and
the queue is not cleared, so a network blip does not interrupt what is on
screen. Only an explicit Stop, a daemon shutdown, or mpv exiting on its own
(`MpvEvent::Exited`) tears mpv down.

## Shutdown

1. `shutdown` latches.
2. `run_session`'s first `select!` arm to see it returns `Ok(())`; the outer
   `run` loop's `match` treats that as "done" and breaks instead of
   reconnecting.
3. `run` calls `rt.stop_playback(true)`: reports Stopped, `quit_and_wait`s mpv,
   clears the queue and the per-mpv-session track state (not the remembered
   tracks — see `specs/tracks.md`).
4. `quit_and_wait` escalates — IPC `quit`, 3 s, `SIGTERM`, 2 s, `SIGKILL` — then
   removes the IPC socket.
5. `instance::listen_stop` breaks its loop and unlinks `stop.sock`.
6. The process exits; the kernel releases the `flock` on `instance.lock`.

Step 6 is why `instance::is_running` probes the lock rather than the socket
file: a `SIGKILL` leaves `stop.sock` behind, so an `exists()` check stayed true
forever and `jellysink update` then chose the stop path and failed with
"connecting to the running instance".

## Known limits

- **The final `Stopped` is best-effort.** The report task is only aborted when
  `run` returns, which happens immediately after `stop_playback` queues that
  last report — so it can still be cancelled before Jellyfin actually receives
  it. Jellyfin reaps the session on its own timeout regardless.
- **Reports are queued unboundedly.** A server that stops answering while
  playback continues accumulates one progress report per second until the
  daemon shuts down.
- **An expired token retries forever.** The daemon stays up and polls every
  60 s rather than exiting, so a systemd unit does not flap; the cost is a
  warning line per minute until someone logs in again.
- **One thread.** `current_thread` flavour: a blocking call anywhere stalls
  keepalives, progress reports and mpv IPC together.
- **A wedged mpv costs 10 s per command.** That is the IPC timeout, and several
  paths issue commands in sequence.
- **`ForceKeepAlive` is trusted.** Whatever the server sends becomes the
  interval, floored at 1 s.

## Tests

Backoff (`src/runtime/session_test.rs`):

- `a_quick_failure_keeps_the_grown_backoff`
- `a_healthy_session_resets_the_backoff`
- `an_expired_token_backs_off_to_the_maximum_however_long_the_session_ran`

The latch (`src/signal_test.rs`) — these pin the `notify_waiters` bug directly:

- `a_signal_fired_before_anyone_waits_is_not_lost`
- `dropping_a_fired_future_does_not_consume_the_latch`
- `take_clears_the_latch_so_the_next_edge_is_a_fresh_wait`
- `clones_share_one_latch`

Auth (`src/jellyfin/auth_test.rs`): `auth_expired_is_recognised_through_added_context`,
`an_unrelated_error_mentioning_401_is_not_an_auth_failure`.

End-file gating (`src/runtime/window_test.rs`):
`end_file_is_ignored_while_replacing_the_current_file`,
`end_file_eof_always_tries_the_next_item`, `end_file_quit_or_error_stops`,
`playlist_jump_stop_is_not_a_session_stop`.

IPC plumbing (`src/mpv/mod_test.rs`): `abandoned_requests_are_evicted`,
`property_coercions_reject_a_missing_or_wrong_typed_answer`,
`end_file_reasons_parse_to_their_variants`.
