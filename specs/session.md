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
| `runtime::run` | `runtime/session.rs`        | The process. Owns the reconnect loop and the report sink.                        |
| `run_session`  | `runtime/session.rs`        | One WebSocket connection over the shared `Runtime`.                              |
| `Runtime`      | `runtime/state.rs`          | The whole daemon session. Owns the queue, mpv and the track memories.            |

**`Runtime` is built once, in `run`, before the reconnect loop starts, and the
same value is threaded into every `run_session` call as `&mut rt`.** A dropped
WebSocket does not rebuild it: mpv keeps playing, the queue and the track
memories are untouched, and `run_session`'s exit is just an `Err` the loop
reconnects from. Only the socket, its reader and the two
`tokio::time::interval`s are per-connection.

The process runs on a `current_thread` runtime built in `main`: one thread,
everything cooperatively scheduled. Blocking-pool threads run on 256 KiB stacks,
so a `spawn_blocking` closure keeps large buffers and deep recursion off the
stack; `main` says why.

## Tasks and channels

| Task              | Produces                             | Spawned by                       | Lifetime                            |
| ----------------- | ------------------------------------ | -------------------------------- | ----------------------------------- |
| Report sink       | `reported` events; consumes `report_tx` | `run`                         | The daemon session.                 |
| mpv IPC loop      | `MpvEvent`s and command replies      | `MpvSession::spawn`              | One mpv process; its `MpvSession`.  |
| WebSocket reader  | `ws_rx` (`WsIncoming`)               | `run_session`                    | One WebSocket connection.           |
| Update check      | nothing; badges the tray             | `cmd_run` (`spawn_update_check`) | Detached; ends after one check.     |
| Tray update apply | nothing; consumes `apply`            | `cmd_run`                        | Detached; the process.              |
| Control listener  | accepts `control.sock` into a `JoinSet` of connection tasks | `cmd_run`         | Detached; until `shutdown`.         |
| Control connection | status, events and replies to one client | `listen_control`             | Its `JoinSet`; until the client leaves or the listener ends. |
| MPRIS emitter     | `PropertiesChanged`/`Seeked` from `status_rx` | `mpris::start`          | Detached; the D-Bus connection.     |

Three channels cross layers, all created in `cmd_run`:

- `status_tx`/`status_rx`, a `watch` of `PlayerStatus`, written by `Runtime` and
  read by each control connection and by MPRIS;
- `event_tx`, a `broadcast` of `Event` (`EVENT_CAPACITY`, 64), written by
  `Runtime` and the report sink and subscribed by each control connection;
- `command_tx`/`command_rx`, unbounded `CastCommand` (a `CastEvent` and its
  `Origin`), written by MPRIS and the control connections and read by
  `run_session`.

Only the WebSocket reader is session-scoped. The report sink is created once in
`run`, before the reconnect loop, precisely so a reconnect does not have to
re-plumb it. mpv's events belong to its `MpvSession` rather than to either: they
follow the mpv process, not the WebSocket.

**Every task spawned inside `runtime` and `mpv` is wrapped in an `AbortOnDrop`**
(`runtime/task.rs`) — there is no collecting struct, each owner holds its own
handle. Without it a reconnect spawns a fresh WebSocket reader and leaves the
previous one running; against a half-open TCP connection that never returns, so
it leaks for the life of the process. The four `cmd_run` and `mpris` tasks are
deliberately detached instead: they are process-scoped, and the process ends by
`exec` or exit. The control connections are held in the listener's `JoinSet`, so
they end with it.

`run_session`'s `select!` arms, in the order they appear:

| Arm                | On firing                                                                                                                                         |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `shutdown.fired()` | Return `Ok(())` — ends `run_session`, and the outer loop breaks.                                                                                   |
| `keepalive.tick()` | Send `{"MessageType":"KeepAlive"}`; a send failure ends the session.                                                                               |
| `progress.tick()`  | `tick_progress` — sample mpv and report, once a second.                                                                                            |
| `ws_rx.recv()`     | Dispatch the parsed `WsIncoming`; see below.                                                                                                      |
| `next_mpv_event`   | `on_mpv_event`. Pends forever while no mpv runs; a closed event channel reads as `Exited`, whose handler drops the session.                        |
| `command_rx.recv()` | A `CastCommand` to `Runtime::handle` with its origin. Disabled by `commands_closed` once the senders are gone — MPRIS and the control socket are optional, so a `None` here is not fatal. |

(The outer `run` loop has a second, two-arm `select!`: `shutdown.fired()` or the
backoff `sleep`.)

`ws_rx` carries a parsed `WsIncoming`, not the raw frame — the reader task does
that parsing so `run_session`'s loop never blocks on it. `Cast(CastEvent)` goes
to `Runtime::handle` with `Origin::Jellyfin`; `ForceKeepAlive` rebuilds the keepalive interval;
`KeepAlive` and `Ignored` are no-ops. **A `None` from `ws_rx.recv()` must return
an `Err`.** The reader owns the sender, so `None` means it is gone — the same
condition a closed socket reports. A closed receiver is ready immediately and
forever, so an empty arm body would leave this arm permanently hot and spin the
loop.

## The control socket

`control.sock` speaks newline-delimited JSON, one frame per line of at most 64
KiB. `control::listen_control` accepts in a two-arm `select!` (`shutdown`,
`accept`) and spawns one `serve` task per client into its `JoinSet`, at most
`MAX_CONNECTIONS` (16); the next is accepted and closed at once. Finished tasks
are reaped before that count is taken, so a client that leaves frees its slot.

A connection must open with a `hello` in `PROTOCOL` within `HELLO_TIMEOUT`, or
it is closed (another protocol gets an `error` frame first). It is then sent a
`welcome` and the current status, so a client that connects or reconnects is
correct at once. Its `select!` is `biased`, in this order:

| Arm                   | On firing                                                                                  |
| --------------------- | ------------------------------------------------------------------------------------------ |
| `status_rx.changed()` | Write the latest status. A `watch`, so a slow client gets the newest, never a backlog.     |
| `events.recv()`       | Write the event. `Lagged` drops the client: it reconnects and the snapshot resyncs it.     |
| `lines.next_line()`   | Answer the command: a `reply` with its `id`, or an `error` for a line with no usable `id`. |

The status goes before an event, so a client reading an outcome already has the
state it led to. Every write is bounded by `WRITE_TIMEOUT` (2 s): a client that
stops reading is dropped and never stalls the daemon. `restart` is answered
before the signal fires, so the updater hears it landed before the process
`exec`s. A cast command is refused with an error `reply` while the daemon has no
server session (`PlayerStatus::server_connected`): `run_session` is what reads
`command_rx`, so one accepted then would wait for the next session and play long
after its client gave up. A shutdown closes every connection before `run`'s
final `stop_playback`, so a client sees a load cut short by it as a disconnect
rather than a `play_cancelled`. Nothing in a frame carries the access token (`PlayerStatus::art_url` is
the anonymous image endpoint).

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
gap, and mpv events raised while the socket is down stay queued on the
`MpvSession` for the next `run_session`. Once reconnected, `Runtime::reannounce` resamples mpv
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
timeout in seconds, which the parser floors at 1 (a malformed or absent `Data`
is 60); the arm halves it, `(seconds / 2).max(1)`, and rebuilds the interval on
the spot. Failing to *send* a keepalive ends the session, which is what puts it back
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

`Runtime::snapshot` builds the payload.

## Stale mpv events

mpv is spawned and killed repeatedly — once per `Stop` / next `PlayNow`, not
once per WebSocket session. Its event channel lives on the `MpvSession`, so
dropping the session drops whatever it still had queued: a stale `end-file` from
the mpv that was just replaced cannot reach the loop and advance the queue past
the episode now playing. A channel shared across mpv processes would need every
event tagged with the process it came from to get the same guarantee.

## Signals

`Signal` (`daemon/signal.rs`) is a latching `watch` channel, not
`Notify::notify_waiters`. `notify_waiters` stores no permit — it only wakes
futures already registered — and every receiver here re-creates its future on
each loop iteration, around loop bodies that routinely await mpv IPC with a 10 s
timeout, so a tray Quit landing in that window was
silently dropped. `fired()` is cancel-safe: dropping the future never consumes
the latch.

| Signal     | Fired by                                                                                             | Consumed by                                    |
| ---------- | ---------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| `shutdown` | tray Quit, MPRIS `Quit`, `cmd_run` unconditionally after its top-level `select!`                     | `run_session`'s loop, `control::listen_control`. |
| `restart`  | a control client's `restart` command (any update path), once its reply is written                     | `cmd_run`, which then `exec`s the new binary.   |
| `apply`    | tray **Install update**                                                                              | The update task in `cmd_run`.                   |

`apply` is the one edge-triggered signal, so its consumer calls `take()` to
clear the latch — otherwise the next click spins on a permanently-set latch
instead of running again.

`cmd_run` selects over the session future, SIGTERM, SIGINT and `restart`;
whichever arm resolves, `shutdown.fire()` runs unconditionally right after, so
any way the daemon leaves that `select!` still tells the session loop to stop.
The listener is not an arm, so a tray Quit ends `cmd_run` through
the session future — which returns only once `stop_playback` has finished. The `restart` arm additionally sets a flag and
**awaits the session future** before returning, so mpv is torn down and the
final report sent before the process replaces itself.

## The loading play

`Runtime::loading` (`Option<Load>`: the item, where known, and the `Origin` that
asked) gates every mpv `end-file` (and every `sid`/`aid` change, see
`specs/tracks.md`) while we are the ones replacing the file. Being `Some` is the
gate; `runtime/loading.rs` is the only code that raises or settles it.

| Raised by (`begin_load`)                           | Settled by                                                                                                                                                                    |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `start_current`, `advance_in_mpv`, `play_previous` | `finish_load` in `on_file_loaded` (`play_started`); `fail_load` for a failed prepare, `load_current`, `playlist-next`/`-prev`, adopt or an mpv `error` end-file (`play_failed`); `cancel_load` for a newer load or a Stop, by its origin, and in `stop_playback` for anything else, by `daemon` (`play_cancelled`). |

**Every `play_loading` gets exactly one outcome**, which is what lets a client
wait on its own command with no timeout. Settling publishes the status before it
sends the event, so no client sees an outcome followed by an older status. A
failed load's `error` goes through `redact_api_key`: it comes from error chains
that can carry a stream URL.

It is also what `PlayerStatus::loading` reports, which is why `start_current`
raises it before preparing the item, while no mpv may be running yet to gate:
a client sees a load in progress by it even when the command was not its own
(`specs/tui.md`). `playlist-prev` raises it with no item id: the window
adopts the item only on `file-loaded`.

Nothing else needs gating during a stop: `stop_playback` runs to completion
inside one `select!` arm and drops the `MpvSession`, and its queued events, with
it.

`end_file_action` (`runtime/window.rs`):

| Condition                           | Action                              |
| ----------------------------------- | ----------------------------------- |
| `loading`, reason not `error`       | `Ignore` — we caused this end-file. |
| Reason `eof` or `redirect`          | `Advance`                           |
| Reason `quit`, `stop` or `error`    | `Stop`                              |
| Reason `other`                      | `Ignore`                            |

An `error` is never ours: replacing a file ends the old one with `stop`, so an
`error` while loading is the file we asked for failing to open. That is a
stop whether or not mpv would go on to a queued stub, and with nothing queued no
`file-loaded` would ever settle the load.

**Every step that raises a load must settle it if it fails.** Nothing emits
`file-loaded` after a failed `playlist-next` or `playlist-prev`, so the load
stays up and `end_file_action` `Ignore`s every later end-file: autoplay dead
until the daemon restarts.

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
3. `run` calls `rt.stop_playback(true)`: reports Stopped, `quit`s mpv, clears
   the queue and the per-mpv-session track state (not the remembered tracks —
   see `specs/tracks.md`).
4. `MpvSession::quit` escalates — IPC `quit`, 3 s, `SIGTERM`, 2 s, `SIGKILL` —
   and dropping the session removes the IPC socket.
5. `control::listen_control` breaks its loop, unlinks `control.sock` and drops
   its connections, so the path is gone as soon as nothing answers on it.
   `cmd_run` unlinks again after its `select!`, because nothing awaits that task.
6. The process exits; the kernel releases the `flock` on `instance.lock`.

Step 6 is why `instance::is_running` probes the lock rather than the socket
file: a `SIGKILL` leaves `control.sock` behind, so an `exists()` check stays true
forever. A client connecting to that leftover is refused, which it reports as
"jellysink is not running" rather than as a failure to connect.

## The restart handoff

The unanswered window is the gap between the old image unlinking `control.sock`
and the new image binding it: the old image's mpv escalation (step 4, up to 5 s)
plus the exec and startup. Either side of that gap the daemon refuses rather than
stalls, because the socket path exists only while something is accepting on it —
`listen_control` removes it when its loop ends, and `bind_control_socket`
recreates it. Every connected client sees its connection close; a client
connecting in the gap reports the daemon as not running, indistinguishable from
the `SIGKILL` leftover above and treated the same. Clients reconnect on their own.

`listen_control` is spawned immediately after the bind rather than being a
`select!` arm, so the new image answers from its first await instead of from
behind the tray and mpris's 3 s timeout. Binding alone would not do it: a connect
to a bound socket that nothing accepts succeeds into the backlog and waits, which
costs the client `REPLY_TIMEOUT` and tells it the daemon is not *responding* —
worse than the refusal it would otherwise have had.

Two ways of closing the window instead are rejected:

- **A marker file the client waits on.** The client that hits its own deadline
  deletes a marker the daemon's restart still depends on, and every client
  would need its own blocking retry loop around it.
- **`instance.lock` in place of the socket.** The restart depends on the flock
  being released by `execve` closing the CLOEXEC fd; `_lock` is still alive at
  `exec_updated`. A lock surviving the exec would deny the new image's own
  `InstanceLock::acquire`, since `flock(2)` treats two fds for one file
  independently. `is_running` also *takes* the exclusive lock while probing, so
  clients polling it would race the new image's re-acquire.

## Known limits

- **The final `Stopped` is best-effort.** The report task is only aborted when
  `run` returns, which happens immediately after `stop_playback` queues that last
  report — so it can be cancelled before Jellyfin receives it. Jellyfin reaps the
  session on its own timeout regardless.
- **Reports are queued unboundedly.** A server that stops answering while
  playback continues accumulates one progress report per second.
- **A restart reads as "not running".** Nothing answers `control.sock` between
  the restart decision and the new image's bind. jellytui reconnects on its own;
  `jellysink status` may need re-running.
- **A command can still wait for the next session.** One accepted just before
  the WebSocket drops sits in `command_rx` until `run_session` runs again.
- **A client is only as current as it reads.** One that falls `EVENT_CAPACITY`
  (64) events behind, or whose write blocks for `WRITE_TIMEOUT`, is dropped; it
  reconnects and the snapshot resyncs it.
- **An expired token retries forever.** The daemon stays up and polls every 60 s
  rather than exiting, so a systemd unit does not flap; the cost is a warning
  line per minute.
- **One thread.** `current_thread` flavour: a blocking call anywhere stalls
  keepalives, progress reports and mpv IPC together.
- **A wedged mpv costs 10 s per command.** That is the IPC timeout, and several
  paths issue commands in sequence. It does not delay a client's status, which
  each connection reads off a `watch` receiver while that command is still
  awaiting its reply.
- **`ForceKeepAlive` is trusted.** Whatever the server sends becomes the
  interval, floored at 1 s.

## Where this is tested

Backoff in `runtime/session_test.rs`; the latch in `daemon/signal_test.rs`; the
control socket in `daemon/control_test.rs` and its client in
`crates/core/src/control_test.rs`; the end-file gating in
`runtime/window_test.rs`; the one outcome per load in `runtime/loading_test.rs`
and, against the live server, `runtime/jellyfin_integration_test.rs`;
`AuthExpired` surviving added context in `crates/core/src/jellyfin/auth_test.rs`;
the IPC coercions and end-file reasons in `mpv/ipc_test.rs` and
`mpv/event_test.rs`.
