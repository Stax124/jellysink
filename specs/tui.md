# jellytui

`jellytui` browses Jellyfin in the terminal and starts playback in a running
jellysink. This note covers the decisions the code cannot state for itself: why
it is a client of the daemon rather than a second copy of it, and why its
per-second state comes from a Unix socket rather than the obvious HTTP call.

## It is a remote, not a player

The daemon already registers as a controllable Jellyfin session
(`post_capabilities` at the top of every `run_session`), so the frontend needs
none of the playback machinery — it sends what the web app sends and the server
relays it:

```
jellytui ──HTTP──> Jellyfin ──WebSocket──> jellysink ──> mpv
```

| jellytui       | HTTP                                                        | arrives as           |
| -------------- | ----------------------------------------------------------- | -------------------- |
| Enter on a row | `POST /Sessions/{id}/Playing?PlayCommand=PlayNow&ItemIds=…` | `CastEvent::PlayNow` |

**That row is the whole table, and deliberately so.** The stream plays in the
user's own mpv, which owns pause, seek, volume, mute and fullscreen already,
and the daemon adopts mpv's own playlist navigation for free
(`adopt_playlist_pos`, `specs/playlist.md`) while a closed window is already a
stop. Binding any of them here as well is two sources of truth for state mpv is
authoritative about, and the second one is always the stale one. `cast.rs` still
parses the full vocabulary — the web app, a phone and MPRIS all send it — so
that coverage belongs in `cast_test.rs`, not beside a sender.

The table is session commands, and `t` (`Api::set_played`) is not one: it writes
user data straight to the server and the daemon never sees it, which is also why
it works with nothing playing and no daemon running at all.

The one capability with no mpv equivalent is Next when the following episode is
not yet loaded into mpv's window (`NextNotInMpv`); stubs normally keep an entry
either side, so it is reachable from the web app and rare in practice.

Queue building, series autoplay, remembered tracks and progress reporting stay
the daemon's. The alternative — a `jellysink browse` subcommand owning its own
`Runtime` — is ruled out by `InstanceLock::acquire` being exclusive and
`paths.mpv_socket()` being a fixed path: it could not run beside the daemon most
users have under systemd, and it would mean two ways to be a player.

**Two consequences.** The frontend is useless without a running daemon and says
so rather than failing obscurely (`session_id` sets the "jellysink is not
connected" line). And anything it sends must be something `cast.rs` parses —
the two halves are joined through a third process, so a misspelled command name
fails silently at runtime. `core`'s `jellyfin/remote_test.rs` holds the
`PLAY_NOW` constant against `CastEvent::from_ws` to catch that in CI, and takes
a row per command sent.

## Finding the daemon's session

`GET /Sessions?deviceId={device_id}`, using the device id from `cred.json` —
*not* a match on the client name. A reinstall leaves a stale session behind with
the same `Client` of `"jellysink"`, and commands posted to that one go nowhere.
Sharing the stored credentials means the server sees one session for both
processes, which is what makes "my target is my own session" true; the cost is
that jellytui only drives a jellysink on the same machine.

## Updating

`jellysink update` and the tray's Install update replace the **daemon only**:
`daemon/update.rs` pins the release asset to `jellysink-<target>` by exact name,
because every asset carries the target triple and `self_update`'s default
substring match would otherwise be free to pick `jellytui-<target>` or a
`.sha256`. `install.sh` installs and updates both.

So a self-updated machine can run a newer daemon beside an older `jellytui`.
That is expected to keep working: the two are joined only by the command names
in `cast.rs`, which are Jellyfin's own. If that ever stops being true, the
frontend needs a version check — not a shared updater.

## Staying responsive

Every request runs in a spawned task reporting back over one `mpsc` channel
(`Msg`), so no keystroke waits on HTTP. Three staleness rules follow, and they
are the same rule three times — **the view outlives the thing it points into**:

- **Search** is debounced `SEARCH_DEBOUNCE` and each request carries a
  generation number, so a slow earlier response is dropped rather than
  overwriting a newer one.
- **Level loads** carry their depth. If the user goes back before the rows
  arrive, the depth no longer indexes into the stack and the rows are dropped.
- **`Level::fill`** pulls the cursor back into range when a reload returns fewer
  rows, because the selection outlives the list it points into.

## Where the footer's state comes from

Not from `GET /Sessions`. That response embeds `NowPlayingQueueFullItems` — a
full item DTO per queue entry — and **no request parameter trims it**: `fields`,
`EnableFullItems` and `enableImages` were all measured against a real server and
changed nothing. With a 103-episode series queued it is **2.4 MB**, and it stays
that large after playback stops because the queue persists. Polled once a second
on a `current_thread` runtime, that is megabytes of download, TLS decryption and
JSON parsing per second on the thread that draws the UI.

So the footer polls the daemon's own status socket (`instance::request_status`,
the one `jellysink status` uses): **79 bytes**, no TLS, no JSON tree, and it
carries the queue position `/Sessions` made us compute. The blocking call runs
in `spawn_blocking` so a wedged daemon cannot stall the loop. `/Sessions` is
still needed for the session id that addresses commands, but that is fetched
**once** in the background at startup and cached.

Two things follow:

- The status carries the duration alongside the position, so the progress bar
  needs no request of its own and no key to guard against a stale total.
- The footer's title is the daemon's `display_title`, not `Item::label`, so it
  reads the same as the mpv window title regardless of who started playback —
  which also means the footer works for playback started from a phone or the web
  app.

Seeking is computed from the last polled position, so it can be up to a second
stale; invisible at ten-second steps.

## Reloading on a playback change

Browse rows are fetched once, when the level is opened, so an episode watched
through jellysink leaves every view that names it wrong — an unticked row, a
resume point that has moved, a Next Up that has advanced a week. The poll above
already carries the answer: `on_player` compares the polled item id with the
previous one, and any difference is a playback change. `Some` → `None` is a
stop or a closed mpv window, `Some(a)` → `Some(b)` an episode handing over,
`None` → `Some` a start — one comparison covers all three, so there is nothing
to subscribe to and no second connection to keep up.

**The reload is deferred by one poll.** `stop_playback` queues its `Stopped`
report onto the report sink and calls `publish_status` without waiting for the
POST, so at the instant jellytui can see the change the server may still answer
with the watched state that report is about to replace. The change therefore
arms `reload_due` and the *following* poll fires it, which is why `on_player`
fires before it arms — doing it the other way round collapses the deferral to
nothing. A second is free here: the poll ticks anyway, so this costs a `bool`
and no timer.

The first poll is exempt, by the same `player_polled` guard the daemon-connected
log line uses: startup has just loaded Home, and finding something already
playing is not news about it.

What reloads is the current screen plus Home, always — Continue Watching and
Next Up are what finishing an episode invalidates, and they are rarely the
screen that was up when it finished. `refresh` (the `r` key) is the same
`reload_current_screen` call followed by a poll, so the two cannot drift.

`reload_screen_and_home` has a second caller: a successful `t` reaches it too,
because marking something watched invalidates exactly what finishing it does.
There the reload is what makes the tick appear, so it is not deferred — the
server has already answered.

The cost is one duplicate pair of requests when the Playing screen is up as an
episode hands over: the existing `load_playing` chain fires on the change
itself. That is deliberate — the immediate fetch is what fills the Playing
screen, and the deferred one is what corrects the episode list's watched marks,
which the immediate fetch reads too early.

## Where a complaint goes

The bottom row is the key bindings and nothing else — a message there hides
every binding the user might need to recover with. `App::message` (a failed
request, a command sent before the session id landed) is drawn in the
**header**, between the tabs and the daemon dot, elided by `view::to_width`, and
retired by the next keypress.

**Every row has to fit 80 columns**, and a test holds each screen to it.
`render_hint` draws a `Paragraph` with no wrap, so an overrun is cut mid-word
with no ellipsis and the bindings nearest the end stop existing for the user —
`q quit` sits at the end of most rows. That budget is why the rows name arrows
rather than `h`/`j`/`k`/`l`, and why none of them names `/`: the vim keys are
bound and Search is a header tab, and spending columns on a second spelling of a
key that already has one buys nothing. `g`/`G` is the
exception and stays visible in the log pane, because no arrow reaches top or
bottom.

The dot (`view::daemon_status`) answers the one question the whole frontend
rests on: did the status socket reply. Green for a daemon that answered, red for
one that did not, and **grey until the first poll returns** — showing red for
that first second would be a lie about a daemon that is fine. A successful play
is not announced at all: the footer reporting it a poll later, in the daemon's
own wording, is the honest confirmation that the command arrived.

## Two view modes

A level is a grid of covers or a list with a detail rail, chosen by **item kind,
not by `Source`** (`nav::is_grid`): `Series`, `Season`, `Movie`, `BoxSet` and a
library carry artwork and get the grid, everything else stays a list. By kind, a
folder full of movies gets the grid whichever route reached it, and it is one
function to test rather than a table kept in step with the browse stack. Only
the first row is asked, so kinds that share a screen have to answer alike —
`CollectionFolder` and `UserView` both come back from `/UserViews`, and a
Libraries screen that changed shape with its sort order would be the bug.

Two screens override it. **Search** is always a list — its rows are mixed kinds,
so a grid would be tiles of three different shapes. **Home** is two grids,
Continue Watching above Next Up, each owning half the body and holding a single
row, which is why `grid::metrics` takes the rows it is asked to fill rather than
assuming `TARGET_ROWS`. A shelf scrolls horizontally, so the `offset` that
scrolls a level by rows scrolls a shelf by screenfuls of one.
`App::visible_covers` asks for the tiles in both shelves while `grid_metrics`
answers for the focused one only, so Home does not go through it.

The list beside a rail is split by share rather than a fixed width
(`rail::split`): half each, because a row is text that elides gracefully while
the cover beside it is the thing worth the width. Below `MIN_BODY_WIDTH` there
is no rail at all.

**Keys.** A grid has a second axis, so `h`/`j`/`k`/`l` and all four arrows move
the cursor while one is focused and `Esc` is the only way back. Up and down move
by a whole row, which is why `App` stores the terminal size each iteration —
both sides call `grid::metrics`. On Home they move between the shelves instead
(`App::move_vertically`), each keeping its own cursor. Left and right mean *only*
that: `keys.rs` stays a pure mapping and `App::apply` drops `Intent::Left`/
`Right` unless a grid is focused, so in a list `Esc` and `Enter` are the single
way back and in.

**What a tile says.** There is no room for the `64%` a list row shows, so the
rule under each cover *is* the progress bar and the caption spends its one row
on what the rule cannot carry — `46 left · ★ 8.0`, with a finished item dropping
the count. Selection is the caption's highlight, which leaves the rule to mean
one thing.

`rail::meta` splits on `kind()` for the same reason: a playable item gives its
runtime, a series or a season counts its children (`2018 · 5 seasons · 103
episodes · 46 left · TV-14`), because a series' own `RunTimeTicks` is the
nominal length of one episode and reads as a claim about the whole show. The
year is `ProductionYear`; `Status` and `EndDate` describe season one and are
worse — Jellyfin reports Slime as `Ended` while its fourth season is dated 2026.
Genres appear only when the response carried them, which is why the episode
listings show none.

That is why `ITEM_FIELDS` asks for `RecursiveItemCount` and `seasons()` passes
it too: a series or a season has `PlaybackPositionTicks` of 0, so its progress
can only come from `UserData.PlayedPercentage`, which the server leaves null
unless the count was requested. `ChildCount` and `Genres` ride along on the same
listing for about 10 KB on a hundred rows.

**Tiles are sized by the height**, not by a fixed width: `grid::metrics` divides
the body into `TARGET_ROWS` and takes the cover width from that, so a big
monitor spends its extra height on bigger covers rather than a fifth row of
thumbnails. Three bounds keep that honest:

- never narrower than `minimum_tile_width` — 18 cells for a 2:3 poster, 26 for a
  16:9 still — below which the artwork is not worth drawing;
- never so wide that a row holds fewer than `MIN_COLUMNS` — which stops a still
  taking a third of a wide screen on its own — unless the level has fewer items
  than that, in which case they spread over their own count, since there is
  nothing left for a wide tile to crowd out;
- and **no budget at all** when the area cannot reach `TARGET_ROWS` even at the
  minimum width, since capping a cover that was never going to fit two rows only
  shrinks the one row that does fit.

The last is why the budget is an `Option`, and why a shelf asked for one row
never takes it: `render` skips a tile taller than its area, so an uncapped shelf
draws nothing rather than something small. A shelf drops the width floor for the
same reason it keeps the cap — height binds it, so widening a tile would only
fit fewer of the same covers. The cover is then `cover::fit` against both the
tile width and that budget, because evening the tiles out across the area hands
each one a few columns more than it asked for and a poster obeying its aspect
would grow out of the height with them.

`Metrics::rows` is what the grid **draws**, not what it could hold: the height
budget always divides by `TARGET_ROWS`, so a level too short to fill the grid
never spends the second row's height on a taller cover, and a level with one row
of items reserves one row of height. Tiles are drawn from the top of the body, which is the only place the
leftover can go — a wide screen caps the tile by width, so the covers cannot
grow into the spare height however it is divided, and centring a block against
rows that were never going to be drawn is what puts a gap above the only row
there is.

## Where the artwork comes from

`ratatui-image` draws the covers, with `Picker::from_query_stdio()` deciding
between kitty, sixel, iTerm2 and halfblocks. That query **has to run before
`view::enter()`**: it writes an escape sequence to stdout and reads the answer
off stdin, which the alternate screen would swallow. When it fails — tmux
without passthrough, a plain xterm — `Picker::halfblocks()` is the fallback, so
every terminal gets a picture rather than a hole.

Images are fetched through `Api::primary_image`, which goes through `Api::get`
and so carries the cached auth header. `jellyfin::url::image_url` is *not* used
here: that one puts the token in the query string because MPRIS hands the URL to
a desktop widget to fetch itself, and it stays JPEG at a fixed 600 px because
WebP is not a safe assumption about someone else's widget. It names a quality
for the same reason the covers do — a measured 265 KB poster came back at 226 KB
under the cap alone, and at 75 KB once the quality was named.

Three things are worth stating because getting them wrong is invisible:

- **The server does the resizing**, bounded to the next 64 px bucket around the
  cell box the cover will be drawn in, and asked for as WebP.
  `maxWidth`/`maxHeight` keep the aspect ratio and cap both sides;
  `fillWidth`/`fillHeight` do neither and hand back a full-height poster however
  short the box is. This is the `/Sessions` lesson at a smaller scale — a rail
  cover is about 40 KB rather than a megabyte.

  The bucket is for the *server*, not for us. Jellyfin has no prepared variants:
  `ImageProcessor` re-encodes on demand and caches under a key including the
  dimensions, format and quality, so an exact `cells × font size` — which moves
  with every resize, font and display scale — walks it through a Skia encode per
  request that is then used once. Rounding up costs a few percent more pixels
  and turns a drag-resize into cache hits. It does not reduce our *request*
  count: `Covers::claim` is keyed on the exact size and cell, so the client still
  asks once per distinct box, and the bandwidth win is WebP's alone.

  Because the bucket is wider than the box, the last downscale happens locally
  on every cover, with `FilterType::Lanczos3` — the default `Nearest` at these
  ratios is visible, worst on the small shelf tiles. `format=Webp` is a request
  rather than a guarantee: the server returns the original untouched when its
  encoder cannot handle the source, or for a GIF, which is why the JPEG decoder
  stays enabled alongside it.
- **A cover's size is part of its identity.** A `Protocol` is encoded against one
  rect at one cell size, so after a terminal resize the cached one is the *wrong*
  encoding, not a stale one. `CoverKey` is `(item id, image tag, size, cell)`,
  and because `visible_covers` is recomputed every loop iteration a resize asks
  for the new size without anything having to notice the resize. The cell is the
  pixel grid below: it catches the case the size cannot, a move to a display of
  another scale that leaves the columns and rows exactly where they were.
- **An item's `Primary` is not always a poster.** `cover::fit` sizes the box
  around what `cover::primary_aspect` answers: the server's own
  `PrimaryImageAspectRatio` where it sent one, otherwise 16:9 for an episode's
  still or a library's banner and 2:3 for a poster. A box reserved for the wrong
  shape shows up as a gap between the picture and the text under it, and a grid
  sizes every tile from its first row, so one mis-shaped library mis-shapes the
  whole screen. The measurement cannot carry it alone — `/UserViews` sends the
  ratio and `/Items` only when asked, and a library the server has no artwork
  for sends none at all — which is why the guess stays and why a
  `CollectionFolder` guesses a banner rather than the poster its kind suggests.

### HiDPI: the measured cell size

Kitty is told an image's **pixel** dimensions (`s=`/`v=` in the transmit) and no
column or row count, so it works out how many cells the placement covers by
dividing those pixels by the terminal's *real* cell size. `ratatui-image`,
meanwhile, encodes and lays out against the cell size the terminal reported over
`CSI 16 t` — once, before the alternate screen, because that query cannot be run
again mid-session. Where the two disagree, every cover lands in the top-left
corner of its box at 1/scale of the size while the box, caption and progress
rule stay where the grid put them. Dragging the window to a display of another
scale is the same disagreement arriving later.

So the cell size is measured, once a loop iteration, rather than trusted from
startup. `cover::cell_size` divides the window's pixel size (`ws_xpixel` /
`ws_ypixel`, which `Backend::window_size` reads out of `TIOCGWINSZ`) by the
grid, and `Covers::set_cell_size` rebuilds the picker at that font size, keeping
the protocol the query settled on — `Picker::from_fontsize` is deprecated in
favour of the query, and 11.x has no way to hand a new font size to a picker it
already returned.

The measurement lives at the head of the loop beside the `terminal.size()` read
rather than in an `Event::Resize` arm, because a change of scale need not be a
resize at all: the compositor can hand the terminal more device pixels for the
same grid. A change drops every entry in the cache — the keys carry the cell, so
those encodings could never be looked up again — and requests still in flight
land under the old key rather than on screen. A measurement within a few percent
of the current cell is the window's padding, counted in the window's pixel size
but not in a cell, and is ignored. A terminal that reports no pixel size (tmux,
a plain xterm) is left with the query's answer.

**Do not encode a cover larger than its box.** `Image::render` draws *nothing at
all* when the protocol's size exceeds its area unless `allow_clipping` is set,
so an over-encoded cover is not a sharper one, it is a missing one. A
scale factor belongs in the picker's font size, where the encoding still comes
out at the size of the box, never in the box itself.

### Fetching

Decode and encode run in `spawn_blocking` — jellytui is a `current_thread`
runtime and both are real CPU work on the thread that draws. The finished
`Protocol` comes back over the existing `Msg` channel, so no `select!` arm was
added.

Fetches are throttled to one batch per `COVER_THROTTLE`, and `Covers::claim`
keeps a resting cursor, and a second visit to the same row, to one request. The
throttle is a rate limit rather than a settling delay: the first change after a
quiet spell goes out **immediately**, because that is a cursor arriving
somewhere, and only changes inside the window wait — otherwise holding `j`
through a 218-item library would fire 218 requests. Waiting ones are scheduled
for when the window opens rather than for 120 ms after the last keypress, and
the last change standing is the one that fires, so a cursor coming to rest is
always fetched. A trailing-edge debounce instead makes every single keypress pay
the full window before its request even starts, against an HTTP round trip of
about the same.

**A still screen is not evidence that every cover on it arrived.** The gate on
starting a batch is `Covers::any_missing` — is anything wanted still waiting on
a cover it may yet get — and not whether the wanted set *changed*. With the
latter, a cover evicted after the cursor came to rest stays lost until the
cursor happens to move again; a drag-resize puts a key per intermediate size
through a `CACHE_CAPACITY`-entry cache, so a slow result for a size nobody wants
any more pushes out one just fetched for the size on screen.

That gate is why **a failed request is final**. It makes the blank tile itself
the thing that starts a request, so a key put back to unsettled by a failure
would be asked for once per throttle window for as long as the screen is open —
against a server refusing connections, a full grid is a batch every 120 ms and a
redraw per failure, forever. `Covers::give_up` therefore files a failure
alongside an image the server does not have: both land in `unavailable`, both
are answered from memory for the rest of the session, and only a display-scale
change clears them. An evicted key was never failed, so eviction is unaffected —
and since the disk cache, re-fetching it is a local read rather than a round
trip. Both a failure and an absent image log at `debug`, because the next one of
these should be readable in the `L` pane rather than inferred from a blank tile.

### The disk cache

The in-memory cache holds `CACHE_CAPACITY` covers and stops there on purpose. A
`Protocol` is a decoded frame encoded against one rect at one cell size, so
holding more is straight memory growth — and every one is invalidated by a
resize or a move to another display scale, which is why none can be persisted.

What is persisted is the layer underneath: the **bytes the server sent**, about
40 KB of WebP each, under `Paths::cover_cache_dir` (a `--config` override takes
the cache with it, so an isolated run stays isolated). A memory miss then costs
a local read and a decode instead of an HTTP round trip, at no extra resident
memory. `cover/disk.rs` owns it and `cover/mod.rs` calls it from inside the
`spawn_blocking` that was already doing the decode — plain `std::fs` on a thread
that is already blocking, rather than a second hop through `tokio::fs`, which
would also cost the daemon a tokio feature it does not need.

Four rules:

- **The key is the bucket, not the box.** An entry is
  `{item_id}-{image_tag}-{bucket_w}x{bucket_h}.img`, rounded through core's
  `bucket_pixels` — the same rounding the request makes. Keying on the exact box
  would store a file per pixel of a window drag while the requests behind them
  were all the same one. The key is its own filename and nothing is hashed; the
  id and the tag are sanitised on the way in, because they come from the server
  and a `/` or a `..` in one would make the key a path.
- **Absence stays in memory.** `Covers::unavailable` answers for the session; a
  negative entry on disk would need an invalidation rule of its own for no gain.
- **Nothing here may fail a cover.** Every path in `disk.rs` degrades to a miss,
  and a miss is a fetch. A file that will not decode — a half-write that survived
  a kill — is removed rather than re-decoded on every scroll.
- **Eviction is LRU by mtime**, under `cover_cache_mb` (default 256, `0` off). A
  read touches the file, so what is being looked at outlives what was merely
  fetched first. A write prunes inline once it has written a quarter of the
  budget since the last prune, which is what bounds a long browse; `main.rs`
  prunes once at startup as well, so a budget the user has just lowered takes
  effect even in a session that never writes a cover. A prune goes to 90% of
  budget rather than exactly to it, or a full cache would prune again on the
  next cover.

## The Playing screen

`3` opens a screen for whatever the daemon is on: a banner carrying the
episode's own still with the title, meta, rating and synopsis beside it, then a
rule, then the rest of the season with the playing episode accented and `Enter`
free to jump to another one. Below `MIN_BANNER_WIDTH` the still is dropped and
the text takes the whole width, the way the rail vanishes rather than crowding
the list — `playing::still_rect` returns `None` and `App::visible_covers` asks
for nothing.

The status socket carries a title and a position, not a synopsis or a season, so
this screen makes two requests of its own — `/Items/{id}`, then that episode's
season. Both are keyed by the item id they were asked for and dropped if
playback has moved on. Until they land the daemon's `display_title` holds the
screen, so it is never blank.

## The log pane

`L` opens it and `L` or Esc closes it. It is not in the tab strip: the strip
names it only while it is up, because it is a debugging screen rather than
somewhere to browse.

`logs::install` builds the subscriber in `main`, before the alternate screen is
taken and while a bad filter can still be reported on the normal one. It is
an `EnvFilter` plus one `Layer` writing `LogLine`s into a bounded `VecDeque` — no
`fmt` layer, so nothing can reach stdout. The filter comes from `core`'s
`log_filter`, so `RUST_LOG` means here what it means for the daemon, and
`jellysink_core=debug` turns on the Jellyfin HTTP layer's own events too. The
daemon's logs are **not** here: it is another process, and `stop.sock` has no
command that would carry them.

Levels are split so the default `info` is already worth reading: `info` is what
the user did that had an effect (a play, a command sent, a search, a level
opened) plus every request's elapsed time and row count; `debug` adds a line per
keypress; `trace` adds the once-a-second poll and the per-cover fetch/decode
timings. The 1 Hz poll is deliberately not at `info` — at that rate it would push
everything else out of a 2000-line buffer inside half an hour. What is logged at
`info` is the *transition*, connected to not and back.

Scrolling is anchored by sequence number, not by index: the ring evicts from the
front, so an index into it slides under a paused reader. `Ring` counts
`first_seq` past everything it has dropped and `log_anchor` holds the sequence
number of the top visible line — the same rule search generations and level
depths follow. Reaching the bottom clears the anchor rather than pinning it,
because a view pinned at the tail would stop following the moment the next line
arrived.

## Terminal ownership

mpv is a separate process with its own window and all three stdio handles on
`/dev/null`, so it never contends for the terminal. Two rules follow from owning
the alternate screen:

- `jellytui` never calls `init_tracing`, which builds a `fmt` layer on stdout.
  It installs a subscriber of its own whose only sink is memory.
- `view::enter` installs a panic hook that restores the terminal before
  delegating, so a panic (or a color_eyre report) does not leave the user in a
  raw-mode alternate screen.

`crossterm::event::read` blocks, so input is read on a dedicated OS thread and
forwarded over a channel into the `select!` loop. That thread is detached: at
exit it is parked in `read`, and the process is going away anyway.
