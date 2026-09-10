# jellytui

`jellytui` browses Jellyfin in the terminal and starts playback in a running
jellysink. This note covers the decisions the code cannot state for itself:
why it is a client of the daemon rather than a second copy of it, and why its
per-second state comes from a Unix socket rather than the obvious HTTP call.

## It is a remote, not a player

The daemon already registers as a controllable Jellyfin session
(`Api::post_capabilities` → `POST /Sessions/Capabilities/Full`, at the top of
every `run_session`), and `crates/core/src/cast.rs` already parses the whole remote-control
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
misspelled command name fails silently at runtime.
`crates/core/src/jellyfin/remote_test.rs` round-trips every command through `CastEvent::from_ws` to catch that in CI.

## Finding the daemon's session

`GET /Sessions?deviceId={device_id}`, using the device id from `cred.json` —
*not* a match on the client name. A reinstall leaves a stale session behind with
the same `Client` of `"jellysink"`, and commands posted to that one are
delivered nowhere. Sharing the stored credentials means the server sees one
session for both processes, which is what makes "my target is my own session"
true; the cost is that jellytui only drives a jellysink on the same machine.

## Updating

`jellysink update` and the tray's Install update replace the **daemon only**:
`crates/jellysink/src/daemon/update.rs` pins the release asset to
`jellysink-<target>` by exact name, because every asset of a release carries
the target triple in its name and
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
so a grid would be tiles of three different shapes. **Home** is two grids,
Continue Watching above Next Up, each owning half the body and each holding a
single row of tiles — which is all half a body has the height for, and is why
`grid::metrics` takes the rows it is being asked to fill rather than assuming
`TARGET_ROWS`.

A shelf scrolls horizontally, so the same `offset` that scrolls a level by rows
scrolls a shelf by screenfuls of one. Both shelves are drawn whether or not
they have focus, which the border colour and the caption highlight carry, and
`App::visible_covers` asks for the tiles in both — `grid_metrics` answers for
the focused one only, so Home does not go through it.

The list beside a rail is split by share rather than by a fixed width
(`rail::split`): half each, because a row is a line of text that elides
gracefully while the cover beside it is the thing worth the width. Below
`MIN_BODY_WIDTH` there is no rail at all — half of a narrow body leaves the list
too narrow to read — and the screen stays the full-width list.

Because a grid has a second axis, `h`/`j`/`k`/`l` and all four arrows move the
cursor while one is focused, and `Esc` is the only way back. Up and down move by
a whole row, so `App` needs the column count outside a draw — which is why it
stores the terminal size each iteration and both sides call `grid::metrics`. On
Home there is no row to move down to, so up and down move between the shelves
instead (`App::move_vertically`), each keeping the cursor it was left on; `Tab`
still toggles.

Left and right mean *only* that. `keys.rs` stays a pure mapping — it emits
`Intent::Left`/`Right` and `App::apply` drops them unless `grid_metrics()` says
a grid is focused — so in a list the arrows do nothing and `Esc` and `Enter` are
the single way back and in. They used to double as back and open there, which
made `←` a second, unadvertised way to leave a level.

A grid tile has no room for the `64%` a list row shows, so the shelf rule under
each cover *is* the progress bar and the caption spends its one row on the two
facts the rule cannot carry: how many children are still unwatched, and the
community rating — `46 left · ★ 8.0`. A finished item drops the count rather
than showing a zero, so nothing needs a tick to repeat what the full rule
already says. Selection is carried by the caption's highlight, which leaves the
rule free to mean one thing.

`rail::meta` — the line under the title in the rail and on the Playing screen —
splits on `kind()` for the same reason. A playable item gives its runtime; a
series or a season counts its children instead (`2018 · 5 seasons · 103
episodes · 46 left · TV-14`), because a series' own `RunTimeTicks` is the
nominal length of one episode and reads as a claim about the whole show. The
year is `ProductionYear`, which on a long-running series is the year of its
first season — `Status` and `EndDate` describe season one too and are worse:
Jellyfin reports Slime as `Ended` while its fourth season is dated 2026, so
neither is shown. Genres get their own line when the response carried any,
which is why the episode listings, which do not ask for them, show none.

That rule is why `ITEM_FIELDS` asks for `RecursiveItemCount` and `seasons()`
passes it too. A series or a season has a `PlaybackPositionTicks` of 0, so its
progress can only come from `UserData.PlayedPercentage` — and the server
leaves that null unless the count was requested. Without it every folder drew
an empty rule while its `RunTimeTicks` reported the nominal length of one
episode. `ChildCount` and `Genres` ride along on the same listing: together
they cost about 10 KB on a hundred rows, which is the price of the rail
knowing how many seasons a show has.

Tiles are sized by the **height**, not by a fixed width. `grid::metrics`
divides the body into `TARGET_ROWS` rows and takes the cover width from that,
so a big monitor spends its extra height on bigger covers and captions that
elide less, rather than on a fifth row of thumbnails. Three bounds keep that
honest:

- never narrower than `minimum_tile_width` — 18 cells for a 2:3 poster, 26 for
  a 16:9 still, below which the artwork stops being worth drawing;
- never so wide that a row holds fewer than `MIN_COLUMNS`, which is what stops
  a still (nearly three times the width of a poster at the same height) from
  taking a third of a wide screen on its own;
- and **no budget at all** when the area is too short to reach `TARGET_ROWS`
  even at the minimum width. Capping a cover that was never going to fit two
  rows only shrinks the one row that does fit, so a small terminal keeps the
  tiles it has.

The last one is why the budget is an `Option` — and why a shelf, asked for one
row, never takes it: a tile taller than its area is skipped by `render`, so an
uncapped shelf would draw nothing rather than something small. A shelf drops
the width floor for the same reason it keeps the cap. Height is what binds it,
so its cover is already as large as it can be and a tile widened to
`minimum_tile_width` would hold the same cover while fitting fewer of them. The cover is then `cover::fit`
against both the tile's width and that budget, because evening the tiles out
across the area hands each one a few columns more than it asked for and a
poster obeying its aspect would grow out of the height with them.

## Where the artwork comes from

`ratatui-image` draws the covers, with `Picker::from_query_stdio()` deciding
between kitty, sixel, iTerm2 and halfblocks. That query **has to run before
`view::enter()`**: it writes an escape sequence to stdout and reads the answer
back off stdin, which the alternate screen would swallow. When it fails —
tmux without passthrough, a plain xterm — `Picker::halfblocks()` is the
fallback, so every terminal gets a picture rather than a hole.

Images are fetched through `Api::primary_image`, which goes through `Api::get`
and so carries the cached auth header. `jellyfin::url::image_url` is *not* used
here: that one puts the token in the query string because MPRIS hands the URL
to a desktop widget to fetch itself. It is capped at a fixed 600 px rather than
bucketed, and stays JPEG — the widget fetching it is someone else's, and WebP
is not a safe assumption about it, and it names a quality for the same reason
the covers do. The cap alone was not enough: a measured 265 KB poster came back
at 226 KB until the quality was named too, and then at 75 KB.

Three things are worth stating because getting them wrong is invisible:

- **The server does the resizing**, bounded to the next 64 px bucket around the
  cell box the cover will be drawn in, and asked for as WebP.
  `maxWidth`/`maxHeight` keep the aspect ratio and cap both sides;
  `fillWidth`/`fillHeight` do neither and hand back a full-height poster
  however short the box is. This is the `/Sessions` lesson above at a smaller
  scale — a rail cover is about 40 KB rather than a megabyte.

  The bucket is for the *server*, not for us. Jellyfin has no prepared
  variants: `ImageProcessor` re-encodes on demand and caches under a key that
  includes the dimensions, the format and the quality, so an exact `cells ×
  font size` — which moves with every resize, font and display scale — walks it
  through a Skia encode per request that is then used once. Rounding up costs a
  few percent more pixels and turns a drag-resize into cache hits. It does not
  reduce our *request* count: `Covers::claim` is keyed on the exact size and
  cell, so the client still asks once per distinct box, and the bandwidth win
  is WebP's alone.

  Because the bucket is wider than the box, the last downscale now happens
  here, on every cover, where it used to be a no-op. `Resize::Fit` never
  upscales, so the surplus is resampled locally — with `FilterType::Lanczos3`,
  because the default `Nearest` at these ratios is visible, worst on the small
  shelf tiles. `format=Webp` is a request rather than a guarantee: the server
  returns the original untouched when its encoder cannot handle the source, or
  for a GIF, which is why the JPEG decoder stays enabled alongside it.
- **A cover's size is part of its identity.** A `Protocol` is encoded against
  one rect at one cell size, so after a terminal resize the cached one is the
  *wrong* encoding, not a stale one. `CoverKey` is `(item id, image tag, size,
  cell)`, and because `visible_covers` is recomputed every loop iteration, a
  resize asks for the new size without anything having to notice the resize.
  The cell is the pixel grid below (see *HiDPI*): it catches the case the size
  cannot, a move to a display of another scale that leaves the columns and rows
  exactly where they were.
- **An item's `Primary` is not always a poster.** Every cover the frontend
  draws is a `Covers::key` on the item's `Primary`, so a row's shape follows
  it, and `cover::fit` sizes the box around what `cover::primary_aspect`
  answers: the
  server's own `PrimaryImageAspectRatio` where it sent one, otherwise 16:9 for
  an episode's still and 2:3 for a poster. Guessing is not enough on its own —
  a library's primary image is a 16:9 banner although a `CollectionFolder`
  looks like a poster by kind, and the box reserved for the wrong shape shows
  up as a gap between the picture and the text under it. `/UserViews` sends the
  ratio, `/Items` only when asked, which is why the guess stays. The Playing
  screen used to ask for the *series* poster instead
  (`SeriesPrimaryImageTag`); it now shows the episode's own still, which is
  what the rail beside a season already showed.

### HiDPI: the measured cell size

Kitty is told an image's **pixel** dimensions (`s=`/`v=` in the transmit) and no
column or row count, so it works out how many cells the placement covers by
dividing those pixels by the terminal's *real* cell size. `ratatui-image`,
meanwhile, encodes and lays out against the cell size the terminal reported over
`CSI 16 t` — once, before the alternate screen, because that query cannot be run
again mid-session. Where the two disagree, every cover lands in the top-left
corner of its box at 1/scale of the size; the box, caption and progress rule
stay where the grid put them. Dragging the window to a display of another scale
is the same disagreement arriving later: the reported cell size is frozen at
startup, and encoding against it after the move draws every cover at the wrong
fraction of its box, in whichever direction the move went.

So the cell size is measured, once a loop iteration, rather than trusted from
startup. `cover::cell_size` divides the window's pixel size — `ws_xpixel` /
`ws_ypixel`, which `Backend::window_size` reads out of `TIOCGWINSZ` — by the
grid, and `Covers::set_cell_size` builds the picker again at that font size,
keeping the protocol the query settled on (`Picker::from_fontsize` is deprecated
in favour of the query, and 11.x has no way to hand a new font size to a picker
it already returned). A cover is then fetched and encoded for `cells × the real
cell`, so kitty measures the placement out to exactly the box the grid drew.

The measurement lives at the head of the loop beside the `terminal.size()` read
rather than in an `Event::Resize` arm, because a change of scale need not be a
resize at all — the compositor can hand the terminal more device pixels for the
same grid — and the once-a-second poll then bounds how long a cover can stay
encoded for the old pixel grid. A change drops every entry in the cache; the
keys carry the cell, so those encodings could never be looked up again, and the
requests still in flight land under the old key rather than on screen.

**Do not encode a cover larger than its box.** `image_scale` used to do exactly
that — a user-typed multiplier on the cell box, on the understanding that the
widget clamped the cells it drew and the surplus was spent on pixels. It does
not: `Image::render` draws *nothing at all* when the protocol's size exceeds its
area unless `allow_clipping` is set, so an over-encoded cover is not a sharper
one, it is a missing one. That is why the factor belongs in the picker's font
size, where the encoding still comes out at the size of the box, rather than in
the box. A measurement within a few percent of the current cell is the window's
padding — it is counted in the window's pixel size but not in a cell — and is
ignored, or every pixel the window moved would cost a round trip.

The key went with the mechanism. The number was the terminal's to report all
along, and a fixed one is wrong the moment the window changes display. A
terminal that reports no pixel size — tmux, a plain xterm — is left with the
query's answer, which is what it drew with before.

Decode and encode run in `spawn_blocking` — jellytui is a `current_thread`
runtime and both are real CPU work on the thread that draws. The finished
`Protocol` comes back over the existing `Msg` channel, so no `select!` arm was
added. Fetches are throttled to one batch per 120 ms, and `Covers::claim` keeps
a resting cursor, and a second visit to the same row, to one request. The
throttle is a rate limit rather than a settling delay: the first change after a
quiet spell goes out **immediately**, because that is a cursor arriving
somewhere, and only changes inside the window wait — otherwise holding `j`
through a 218-item library would fire 218 requests. Waiting ones are scheduled
for when the window opens rather than for 120 ms after the last keypress, and
the last change standing is the one that fires, so a cursor coming to rest is
always fetched. It was a trailing-edge debounce until it was measured: every
single keypress paid the full 120 ms before its request even started, against
an HTTP round trip of about the same. The cache is bounded and evicts oldest-first;
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

## The log pane

`L` opens it and `L` or Esc closes it again. It is not in the tab strip: the
strip names it only while it is up, because it is a debugging screen rather
than somewhere to browse.

`logs::install` builds the subscriber in `main`, before the alternate screen is
taken and while a bad filter can still be reported on the normal one. It is
`Targets` + one `Layer`, and that layer writes `LogLine`s into a bounded
`VecDeque` — no `fmt` layer, so nothing can reach stdout. The filter comes from
`core`'s `log_filter`, so `log_level` and `RUST_LOG` mean here exactly what they
mean for the daemon, and `jellysink_core=debug` turns on the Jellyfin HTTP
layer's own events too. The daemon's logs are **not** here: it is another
process, and `stop.sock` has no command that would carry them.

Levels are split so the default `info` is already worth reading: `info` is what
the user did that had an effect (a play, a command sent, a search, a level
opened) plus every request's elapsed time and row count; `debug` adds a line per
keypress; `trace` adds the once-a-second poll and the per-cover fetch/decode
timings. The 1 Hz poll is deliberately not at `info` — at that rate it would
push everything else out of a 2000-line buffer inside half an hour. What is
logged at `info` is the *transition*, connected to not and back.

Scrolling is anchored by sequence number, not by index. The ring evicts from the
front, so an index into it slides under a paused reader; `Ring` therefore counts
`first_seq` past everything it has dropped, and `log_anchor` holds the sequence
number of the top visible line. This is the same rule search generations and
level depths follow — the view outlives the buffer it points into. Reaching the
bottom clears the anchor rather than pinning it there, because a view pinned at
the tail would stop following the moment the next line arrived.

## Terminal ownership

mpv is a separate process with its own window and all three stdio handles on
`/dev/null`, so it never contends for the terminal.

Two rules follow from owning the alternate screen:

- `jellytui` never calls `init_tracing`. It builds a `fmt` layer on stdout,
  which is the alternate screen. It installs a subscriber of its own instead —
  see [The log pane](#the-log-pane) — whose only sink is memory.
- `view::enter` installs a panic hook that restores the terminal before
  delegating, so a panic (or a color_eyre report) does not leave the user in a
  raw-mode alternate screen.

`crossterm::event::read` blocks, so input is read on a dedicated OS thread and
forwarded over a channel into the `select!` loop. That thread is detached: at
exit it is parked in `read`, and the process is going away anyway.
