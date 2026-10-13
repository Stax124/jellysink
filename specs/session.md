# The daemon session

How the `run` daemon is wired: ownership, what survives a reconnect, and the ordering rules that stop concurrent events producing the wrong playback state. Almost every rule exists because breaking it caused a specific bug.

## Layers

| Layer          | Lives in             | Lifetime                                                              |
| -------------- | -------------------- | --------------------------------------------------------------------- |
| `cmd_run`      | `cli/run.rs`         | The process. Owns the lock, tray, signals.                            |
| `runtime::run` | `runtime/session.rs` | The process. Owns the reconnect loop and the report sink.             |
| `run_session`  | `runtime/session.rs` | One WebSocket connection over the shared `Runtime`.                   |
| `Runtime`      | `runtime/state.rs`   | The whole daemon session. Owns the queue, mpv and the track memories. |

- **`Runtime` is built once in `run`, before the reconnect loop, and passed to every `run_session` as `&mut rt`.** A dropped WebSocket leaves mpv, the queue and the track memories untouched; only the socket, its reader and the two intervals are per-connection.
- **One `current_thread` runtime.** Blocking-pool threads have 256 KiB stacks, so `spawn_blocking` closures keep large buffers and deep recursion off the stack.

## Tasks and channels

| Task               | Produces                                                    | Spawned by                       | Lifetime                                                     |
| ------------------ | ----------------------------------------------------------- | -------------------------------- | ------------------------------------------------------------ |
| Report sink        | `reported` events; consumes `report_tx`                     | `run`                            | The daemon session.                                          |
| mpv IPC loop       | `MpvEvent`s and command replies                             | `MpvSession::spawn`              | One mpv process; its `MpvSession`.                           |
| WebSocket reader   | `ws_rx` (`WsIncoming`)                                      | `run_session`                    | One WebSocket connection.                                    |
| Update check       | nothing; badges the tray                                    | `cmd_run` (`spawn_update_check`) | Detached; ends after one check.                              |
| Tray update apply  | nothing; consumes `apply`                                   | `cmd_run`                        | Detached; the process.                                       |
| Control listener   | accepts `control.sock` into a `JoinSet` of connection tasks | `cmd_run`                        | Detached; until `shutdown`.                                  |
| Control connection | status, events and replies to one client                    | `listen_control`                 | Its `JoinSet`; until the client leaves or the listener ends. |
| MPRIS emitter      | `PropertiesChanged`/`Seeked` from `status_rx`               | `mpris::start`                   | Detached; the D-Bus connection.                              |

Cross-layer channels, all created in `cmd_run`:

- `status_tx`/`status_rx` — a `watch` of `PlayerStatus`, written by `Runtime`, read by each control connection and by MPRIS.
- `event_tx` — a `broadcast` of `Event` (`EVENT_CAPACITY`, 64), written by `Runtime` and the report sink, subscribed by each control connection.
- `command_tx`/`command_rx` — unbounded `CastCommand` (a `CastEvent` and its `Origin`), written by MPRIS and the control connections, read by `run_session`.

- **Only the WebSocket reader is session-scoped.** The report sink is created once in `run` so a reconnect need not re-plumb it; mpv's events follow the mpv process through its `MpvSession`.
- **Every task spawned in `runtime` and `mpv` is held in an `AbortOnDrop`** (`runtime/task.rs`) by its owner. Otherwise a reconnect leaks the old reader, which never returns on a half-open TCP connection. The `cmd_run` and `mpris` tasks are deliberately detached as process-scoped, since the process ends by `exec` or exit; control connections end with the listener's `JoinSet`.

`run_session`'s `select!` arms, in order:

| Arm                 | On firing                                                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `shutdown.fired()`  | Return `Ok(())`; the outer loop breaks.                                                                                         |
| `keepalive.tick()`  | Send `{"MessageType":"KeepAlive"}`; a send failure ends the session.                                                            |
| `progress.tick()`   | `tick_progress` — sample mpv and report, once a second.                                                                         |
| `ws_rx.recv()`      | Dispatch the parsed `WsIncoming`; see below.                                                                                    |
| `next_mpv_event`    | `on_mpv_event`. Pends forever while no mpv runs; a closed event channel reads as `Exited`, which drops the session.             |
| `command_rx.recv()` | A `CastCommand` to `Runtime::handle` with its origin. Disabled by `commands_closed` once the senders are gone, which is not fatal. |

The outer `run` loop has a two-arm `select!` of its own: `shutdown.fired()` against the backoff `sleep`.

- **`ws_rx` carries a parsed `WsIncoming`**, parsed by the reader so the loop never blocks on it. `Cast` goes to `Runtime::handle` with `Origin::Jellyfin`; `ForceKeepAlive` rebuilds the keepalive interval; `KeepAlive` and `Ignored` are no-ops.
- **A `None` from `ws_rx.recv()` must return an `Err`.** It means the reader is gone, and a closed receiver is ready forever, so an empty arm spins the loop.

## The control socket

- **Newline-delimited JSON, one frame per line, at most 64 KiB.** `listen_control` selects over `shutdown` and `accept` and spawns one `serve` task per client into its `JoinSet`, up to `MAX_CONNECTIONS` (16). Finished tasks are reaped before counting; one over the limit is closed at once.
- **A connection must open with a `hello` in `PROTOCOL` within `HELLO_TIMEOUT`** (another protocol gets an `error` first), then is sent a `welcome` and the current status.

Its `biased` `select!`, in order:

| Arm                   | On firing                                                                                  |
| --------------------- | ------------------------------------------------------------------------------------------ |
| `status_rx.changed()` | Write the latest status. A `watch`, so a slow client gets the newest, never a backlog.     |
| `events.recv()`       | Write the event. `Lagged` drops the client: it reconnects and the snapshot resyncs it.     |
| `lines.next_line()`   | Answer the command: a `reply` with its `id`, or an `error` for a line with no usable `id`. |

- **Status before events**, so a client reading an outcome already has the state it led to.
- **Every write is bounded by `WRITE_TIMEOUT` (2 s)**; a client that stops reading is dropped rather than stalling the daemon.
- **`restart` is answered before the signal fires**, so the updater hears it landed before the `exec`.
- **Cast commands are refused while `PlayerStatus::server_connected` is false.** Only `run_session` reads `command_rx`, so an accepted command would play long after its client gave up.
- **Shutdown closes every connection before `run`'s final `stop_playback`**, so a load it cuts short reads as a disconnect, not a `play_cancelled`.
- **No frame carries the access token**; `NowPlaying::art_url` is the anonymous image endpoint.

## Reconnecting

```
backoff = reconnect_delay(backoff, session_lasted, auth_expired)
sleep(backoff)
backoff = (backoff * 2).min(BACKOFF_MAX)
```

| Case                                            | Delay                                                |
| ----------------------------------------------- | ---------------------------------------------------- |
| Token expired                                   | `BACKOFF_MAX` (60 s), however long the session ran.  |
| Session lasted ≥ `SESSION_HEALTHY_AFTER` (60 s) | `BACKOFF_MIN` (1 s) — it worked, this was a blip.    |
| Anything else                                   | Keep the current value.                              |

- **The healthy-session reset is load-bearing**: without it a few startup failures pin `backoff` at `BACKOFF_MAX` for the life of the process.
- **Nothing touches `rt` between sessions.** mpv events raised while the socket is down stay queued on the `MpvSession`; once back, `Runtime::reannounce` resamples mpv and sends a fresh Start for a server that dropped the session.
- **A 401 from any endpoint becomes the typed `AuthExpired` in `Api::send`.** `is_auth_expired` walks `err.chain()`, since `downcast_ref` sees only the outermost error and `wrap_err` would hide it. Rejected: matching `"401"` in the formatted chain, which also fires for port 401 or an id containing it.
- **An expired token never exits the daemon**; it logs an error per retry, every 60 s, until `jellysink login` is run, so a systemd unit does not flap.

## Keepalive

- **30 s by default; `ForceKeepAlive` overrides it, trusted as sent.** The parser floors the server's timeout at 1 (60 when `Data` is missing or malformed), and the arm halves it, `(seconds / 2).max(1)`, and rebuilds the interval at once.
- **A failed keepalive send ends the session**, which puts it through backoff.
- **`progress` uses `MissedTickBehavior::Skip` and `keepalive` `Delay`**: a late progress tick is worthless, a late keepalive still has to go.

## Reporting

- **`spawn_report_sink` serialises every `Report` on one FIFO task.** That alone stops a Stopped overtaking the Start before it (`start_current` sends both microseconds apart), and because the task lives in `run` the guarantee holds across reconnects. `Runtime::snapshot` builds the payload.

## Stale mpv events

- **mpv's event channel lives on its `MpvSession`**, which is replaced per Stop or PlayNow, so dropping it drops a stale `end-file` that would advance the queue past the playing episode. A channel shared across mpv processes would need every event tagged with its process.

## Signals

- **`Signal` (`daemon/signal.rs`) is a latching `watch`, not `Notify::notify_waiters`.** `notify_waiters` stores no permit, and receivers here re-create their future each iteration around mpv IPC awaits of up to 10 s, so a Quit in that window would be dropped. `fired()` is cancel-safe.

  | Signal     | Fired by                                                                           | Consumed by                                      |
  | ---------- | ---------------------------------------------------------------------------------- | ------------------------------------------------ |
  | `shutdown` | tray Quit, MPRIS `Quit`, `cmd_run` unconditionally after its top-level `select!`   | `run_session`'s loop, `control::listen_control`. |
  | `restart`  | a control client's `restart` command (any update path), once its reply is written | `cmd_run`, which then `exec`s the new binary.    |
  | `apply`    | tray **Install update**                                                            | The update task in `cmd_run`.                    |

- **`apply` is edge-triggered**: its consumer calls `take()`, or the next click spins on a set latch.
- **`cmd_run` selects over the session future, SIGTERM, SIGINT and `restart`, then fires `shutdown` unconditionally**, so any exit stops the session loop. The listener is not an arm, so a tray Quit ends `cmd_run` through the session future, which returns only after `stop_playback`. The `restart` arm also awaits the session future, so mpv is torn down and the final report sent before the `exec`.

## The loading play

- **`Runtime::loading` (`Option<Load>`: the item where known, and the requesting `Origin`) gates every mpv `end-file` and `sid`/`aid` change while we replace the file.** Only `runtime/loading.rs` raises or settles it.
  - Raised by `begin_load` in `start_current`, `advance_in_mpv` and `play_previous`.
  - `finish_load` in `on_file_loaded` → `play_started`.
  - `fail_load` for a failed prepare, `load_current`, `playlist-next`/`-prev`, adopt, or an mpv `error` end-file → `play_failed`.
  - `cancel_load` for a newer load or a Stop (by its origin), and in `stop_playback` otherwise (by `daemon`) → `play_cancelled`.
- **Every `play_loading` gets exactly one outcome**, so a client can wait on its own command with no timeout. The status is published before the event. A failure's `error` goes through `redact_api_key`, since error chains can carry a stream URL.
- **Every step that raises a load must settle it on failure.** A failed `playlist-next`/`-prev` emits no `file-loaded`, so the load would stay up and every later end-file be ignored, killing autoplay until restart.
- **`start_current` raises it before preparing**, possibly with no mpv yet, because `PlayerStatus::loading` reports it to clients (`specs/tui.md`). `playlist-prev` raises it with no item: the window adopts on `file-loaded`.
- **A stop needs no gate**: `stop_playback` runs within one `select!` arm and drops the `MpvSession` with its queued events.

`end_file_action` (`runtime/window.rs`):

| Condition                        | Action                              |
| -------------------------------- | ----------------------------------- |
| `loading`, reason not `error`    | `Ignore` — we caused this end-file. |
| Reason `eof` or `redirect`       | `Advance`                           |
| Reason `quit`, `stop` or `error` | `Stop`                              |
| Reason `other`                   | `Ignore`                            |

- **An `error` is never ours**: replacing a file ends the old one with `stop`, so an `error` while loading is the requested file failing. It is a Stop even when mpv would go on to a queued stub, and with nothing queued no `file-loaded` would settle it.
- **`EndFileReason` is parsed once in `mpv/event.rs`**, so the match is exhaustive and a typo cannot fall into the ignore arm.
- **A `stop` with `playlist_count > 1` is a `playlist-next` or OSC jump, not a user Stop** (`ignore_stop_for_playlist`); the runtime waits for the `file-loaded` that follows (`specs/playlist.md`).
- **Reading mpv is not optional.** `playlist_state` errors rather than fabricating `(0, 0)`, `play_next_or_stop` stops when it cannot read, and `as_i64_property` and friends (`mpv/ipc.rs`) reject wrong-typed answers. A guessed `playlist-pos` → 0 or `volume` → 100 turns an IPC hiccup into the wrong episode.

## What survives what

| Across…                 | `Runtime` (queue, mpv, track memories)             | Instance lock |
| ----------------------- | -------------------------------------------------- | ------------- |
| A WebSocket reconnect   | **yes — the same `Runtime`, untouched**            | held          |
| `stop_playback`         | yes; queue cleared, mpv killed                     | held          |
| Update restart (`exec`) | no — `run` returns and the process replaces itself | re-acquired   |

Only an explicit Stop, a shutdown, or mpv exiting on its own (`MpvEvent::Exited`) tears mpv down.

## Shutdown

1. `shutdown` latches.
2. `run_session`'s first arm returns `Ok(())`; the outer loop breaks instead of reconnecting.
3. `run` calls `rt.stop_playback(true)`: reports Stopped, quits mpv, clears the queue and per-mpv-session track state (not the remembered tracks, `specs/tracks.md`).
4. `MpvSession::quit` escalates — IPC `quit`, 3 s, `SIGTERM`, 2 s, `SIGKILL` — and dropping the session removes the IPC socket.
5. `listen_control` breaks, unlinks `control.sock` and drops its connections. `cmd_run` unlinks again after its `select!`, since nothing awaits that task.
6. The process exits and the kernel releases the `flock` on `instance.lock`.

- **`instance::is_running` probes the lock, not the socket file**, since a `SIGKILL` leaves `control.sock` behind. A client refused by that leftover reports "jellysink is not running".

## The restart handoff

- **Nothing answers between the old image unlinking `control.sock` and the new one binding it** (mpv escalation up to 5 s, plus exec and startup). Either side the daemon refuses rather than stalls, because the path exists only while something accepts on it; clients see a close or "not running" and reconnect on their own.
- **`listen_control` is spawned right after the bind, not as a `select!` arm**, and ahead of tray and MPRIS startup (MPRIS alone may take 3 s), so the new image answers from its first await. A bound socket nothing accepts on holds connects in the backlog, costing the client `REPLY_TIMEOUT` and reading as "not responding".
- Rejected: **a marker file the client waits on.** A client hitting its deadline deletes a marker the restart depends on, and every client needs its own retry loop.
- Rejected: **`instance.lock` in place of the socket.** The flock is released by `execve` closing the CLOEXEC fd (`_lock` is alive at `exec_updated`); a lock surviving the exec would deny the new image's `InstanceLock::acquire`, since `flock(2)` treats fds independently. `is_running` also takes the lock while probing, racing the re-acquire.

## Known limits

- **The final Stopped is best-effort.** The report task is aborted as soon as `run` returns, right after `stop_playback` queues it; Jellyfin reaps the session on its own timeout.
- **Reports are queued unboundedly.** A server that stops answering during playback accumulates one per second.
- **`jellysink status` may need re-running across a restart**, which reads as "not running" until the new image binds.
- **A command accepted just before the WebSocket drops** waits in `command_rx` for the next session.
- **One thread.** A blocking call anywhere stalls keepalives, progress and mpv IPC together.
- **A wedged mpv costs 10 s (the IPC timeout) per command**, and some paths issue several in sequence. Client status still flows through the `watch`.
