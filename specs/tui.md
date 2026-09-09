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

## Where a complaint goes

The bottom row is the key bindings and nothing else. `App::message` — a failed
request, a command sent before the session id landed — is drawn in the
**header**, between the tabs and the daemon dot, elided by `ui::to_width` to
whatever those two leave, and retired by the next keypress.

The dot itself (`ui::daemon_status`) is the far right of the header and answers
the one question the whole frontend rests on: did the status socket reply.
Green for a daemon that answered, red for one that did not, and **grey until
the first poll returns** — the frontend starts with `player_polled` false, and
showing red for that first second would be a lie about a daemon that is fine.
The three states are the same three the footer spells out in words; the dot is
the version you can read without looking. It lived on the hint row until it was noticed that a
message there hides every binding the user might need to recover with, and that
the commonest one, `Playing …`, only repeated what the footer says a poll later
in the daemon's own wording. So the frontend no longer announces a play at all:
the footer reporting it is the honest confirmation that the command arrived.

## Two view modes

A level is drawn as a grid of covers or as a list with a detail rail, and the
choice comes from **item kind, not from `Source`** (`nav::is_grid`): `Series`,
`Season`, `Movie` and `BoxSet` are poster-shaped and get the grid, everything
else stays a list. Deciding by kind means a folder full of movies gets the
grid whichever route reached it, and it is one function to test rather than a
table that has to be kept in step with the browse stack.

Two screens override it. **Search** is always a list: its rows are mixed kinds,
so a grid would be tiles of three different shapes. **Home** is always a grid,
showing one list at a time with Tab swapping Continue Watching and Next Up in
place — two stacked grids would leave each a single row of tiles, and a
side-by-side carousel would need a horizontal scroll offset of its own.

Because a grid has a second axis, `h`/`j`/`k`/`l` and all four arrows move the
cursor while one is focused, and `Esc` is the only way back. Up and down move by
a whole row, so `App` needs the column count outside a draw — which is why it
stores the terminal size each iteration and both sides call `grid::metrics`.

Left and right mean *only* that. `keys.rs` stays a pure mapping — it emits
`Intent::Left`/`Right` and `App::apply` drops them unless `grid_metrics()` says
a grid is focused — so in a list the arrows do nothing and `Esc` and `Enter` are
the single way back and in. They used to double as back and open there, which
made `←` a second, unadvertised way to leave a level.

A grid tile has no room for the `64%` a list row shows, so the shelf rule under
each cover *is* the progress bar and the caption only says when something is
finished. Selection is carried by the caption's highlight, which leaves the
rule free to mean one thing.

## Where the artwork comes from

`ratatui-image` draws the covers, with `Picker::from_query_stdio()` deciding
between kitty, sixel, iTerm2 and halfblocks. That query **has to run before
`ui::enter()`**: it writes an escape sequence to stdout and reads the answer
back off stdin, which the alternate screen would swallow. When it fails —
tmux without passthrough, a plain xterm — `Picker::halfblocks()` is the
fallback, so every terminal gets a picture rather than a hole.

Images are fetched through `Api::primary_image`, which goes through `Api::get`
and so carries the cached auth header. `jellyfin::url::image_url` is *not* used
here: that one puts the token in the query string because MPRIS hands the URL
to a desktop widget to fetch itself.

Three things are worth stating because getting them wrong is invisible:

- **The server does the resizing**, bounded to the cell box the cover will be
  drawn in. `maxWidth`/`maxHeight` keep the aspect ratio and cap both sides;
  `fillWidth`/`fillHeight` do neither and hand back a full-height poster
  however short the box is. This is the `/Sessions` lesson above at a smaller
  scale — a rail cover is about 40 KB rather than a megabyte.
- **A cover's size is part of its identity.** A `Protocol` is encoded against
  one rect, so after a terminal resize the cached one is the *wrong* encoding,
  not a stale one. `CoverKey` is `(item id, image tag, size)`, and because
  `visible_covers` is recomputed every loop iteration, a resize asks for the
  new size without anything having to notice the resize.
- **An episode's `Primary` is a 16:9 still, not a poster.** Every cover the
  frontend draws is `CoverKey::primary`, so a row's shape follows the item:
  `cover::primary_aspect` answers 16:9 for an episode and 2:3 for everything
  else, and `cover::fit` sizes the box around that. The Playing screen used to
  ask for the *series* poster instead (`SeriesPrimaryImageTag`); it now shows
  the episode's own still, which is what the rail beside a season already
  showed.

Decode and encode run in `spawn_blocking` — jellytui is a `current_thread`
runtime and both are real CPU work on the thread that draws. The finished
`Protocol` comes back over the existing `Msg` channel, so no `select!` arm was
added. Fetches are debounced 120 ms after the visible set settles, or holding
`j` through a 218-item library would fire 218 requests, and `Covers::claim`
keeps a resting cursor to one. The cache is bounded and evicts oldest-first;
items the server has no image for are remembered as absent, because otherwise
an artless row is re-requested on every poll.

## The Playing screen

`3` opens a screen for whatever the daemon is on: a banner carrying the
episode's own still with the title, meta, rating and synopsis beside it, then a
rule, then the rest of the season with the playing episode accented and `Enter`
free to jump to another one. Below `MIN_BANNER_WIDTH` the still is dropped and
the text takes the whole width, the way the rail vanishes rather than crowding
the list — `playing::still_rect` returns `None`, and `App::visible_covers` asks
for nothing.

The status socket carries a title and a position, not a synopsis or a season,
so this screen makes two requests of its own — `/Items/{id}`, then that
episode's season. Both are keyed by the item id they were asked for and dropped
if playback has moved on, the same rule search generations and level depths
follow. Until they land the daemon's `display_title` holds the screen, so it is
never blank.

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
