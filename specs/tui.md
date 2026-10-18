# jellytui

`jellytui` browses Jellyfin in the terminal and starts playback in a running jellysink. This covers what the code cannot say for itself: why it is a client of the daemon, and why it talks to it over a Unix socket.

## A remote, not a player

```
jellytui ──control.sock──> jellysink ──> mpv
```

- **The only key bound to a daemon command is Enter, sending `play_now`** (`{"id": n, "command": "play_now", "item_ids": […], …}`, arriving as `CastEvent::PlayNow` from `Origin::Client`). mpv already owns pause, seek, volume, mute and fullscreen, the daemon adopts mpv's playlist navigation (`specs/playlist.md`), and closing the window is a stop; binding them here too would be a second, stale source of truth.
- **The rest of the `CastEvent` vocabulary is for other controllers**, so its coverage belongs in `cast_test.rs`, not beside a sender.
- **`t` (`Api::set_played`) is not a daemon command**: it writes to the server directly, so it works with no daemon at all.
- **Next to an episode not yet in mpv's window (`NextNotInMpv`) has no mpv equivalent**, so jellytui does not offer it; stubs normally keep an entry either side, and the web app reaches it in the rare case.
- Rejected: **a `jellysink browse` subcommand owning its own `Runtime`.** `InstanceLock::acquire` is exclusive and `paths.mpv_socket()` is fixed, so it could not run beside a systemd daemon, and it would be a second player.
- Rejected: **playing through the server.** Its `PlayNow` has no field for a request id, so jellytui could not tell its own play apart; over the socket every event names its cause. The socket is local, so jellytui drives only a jellysink on the same machine, which shares its `cred.json` anyway.
- **Without a running daemon it says so** (`play` sets "jellysink not connected") rather than failing obscurely.
- **The frame tags and field names are a contract between separately updated binaries**, pinned by `core`'s `control_test.rs` and `cast_test.rs`.

## Updating

- **`core/src/update.rs` is the one updater, parameterised by binary name.** It matches the release asset `<bin_name>-<target>` exactly, because `self_update`'s substring match could pick the other binary or a `.sha256`.
- **Either binary updates both**: itself with `self_replace`, then the `sibling_binary` beside it, unconditionally, since either can be behind. A sibling's baseline is its own `--version`, not the running process's.
- **The sibling install moves through `bin_install_path` and needs an explicit `chmod`**: a bare-binary asset extracts 0644 and only `self_replace` keeps the old mode. A sibling that cannot be replaced is reported, not raised, since this binary's own update has already landed.
- **`--force` pins the comparison baseline to `0.0.0`**, reinstalling the latest over both; it is refused with `--check`.
- **Replacing the daemon's binary does not restart it**: the rename leaves the running process on its old inode. Whichever binary updated sends `restart` on `control.sock` and waits for the reply — `install_both` is the one flow for the tray, `jellysink update` and `jellytui update` — and only when the daemon's binary was replaced, since a restart ends playback.
- **The startup check only badges the header**; a failure is a log line. `App::update` keeps the check's state, so `u` before it answers or after it fails says so instead of calling the binary current.
- **`u` exits the loop rather than downloading behind the TUI**: `App::run` returns an `Exit`, `ratatui::restore()` runs, `main` installs on the normal screen and `exec_updated` brings the TUI back. Rejected: suspending the alternate screen, which leaves the detached input thread parked in `read` holding stdin.
- **A daemon older than jellytui keeps working within one `PROTOCOL`**, since additions are fields and frames either side skips; across a change it reads as "not connected" until restarted. The skew is accepted.

## Staying responsive

Every request is a spawned task reporting over one `mpsc` (`Msg`), so no keystroke waits on HTTP. **A listing fails after `REQUEST_TIMEOUT` (10 s)**, so a hung server cannot leave a screen loading. Rejected: the timeout on core's client, which would also cut the daemon's PlaybackInfo while the server opens a live stream. The staleness rules are one rule — **the view outlives what it points into**:

- **Search** is debounced `SEARCH_DEBOUNCE` and carries a generation, so a slow older response is dropped.
- **Level loads** carry their depth and `Source` and land only on the matching level; depth alone fails after going back and opening another level.
- **`Rows::fill`** pulls the cursor back into range when a reload returns fewer rows.
- **`App::rescroll`** runs at the head of every iteration for each grid on screen, so a reload or resize scrolls the cursor back into view.

**The daemon connection is one task, `player::stay_connected`**, forwarding frames into `Msg` (so the loop has no arm for it) and taking commands over a second channel. On a close or failed connect it sends `Msg::Disconnected` and retries with backoff, 250 ms doubling to 2 s, which rides out a restart. Commands queued on a dead connection are dropped, never replayed.

## The footer's state

- **Rejected: polling `GET /Sessions`.** It embeds a full item per `NowPlayingQueueFullItems` entry and no parameter trims it (`fields`, `EnableFullItems` and `enableImages` were measured); a 103-episode queue is 2.4 MB, even after playback stops, parsed every second on the UI thread.
- **The daemon pushes a `status` frame instead**, about 79 bytes, after every change, carrying the queue position. During playback that is once a second; idle, nothing arrives and nothing redraws. Every connection opens with a snapshot.
- **The status carries the duration**, so the progress bar needs no request of its own.
- **The footer title is the daemon's `display_title`, not `Item::label`**, so it matches the mpv window whoever started playback.

## A play starting

Enter to `file-loaded` takes 2–3 s, mostly mpv's spawn, shown as a spinner fed by two sources because neither covers the wait alone:

- **`PlayerStatus::loading`** is the daemon's `Runtime::loading` (`specs/session.md`). It covers mpv opening the file and plays cast from a phone or the web app.
- **`PendingPlay`** is set on Enter with the request id, since the daemon cannot report a command it has not received, and shows the pressed title until the daemon plays that item.

`PendingPlay` settles only on an event carrying this connection's `client_id` and that request id:

- `play_started` clears it silently;
- `play_failed` puts `jellysink could not play …` in the header;
- `play_cancelled` clears it quietly, since the footer already shows what replaced it;
- a refusing `reply` clears it and puts the daemon's reason in the header (it refuses while it has no server session);
- a disconnect clears it.

- **There is no timeout**: the daemon sends exactly one outcome per `play_loading`, and a dead daemon is a disconnect. Events of other origins only drive the footer.
- **While either source is up the loop wakes every `SPINNER_FRAME` (100 ms)**; otherwise it redraws only on frames and keys.

## Reloading on a playback change

- **A `reported` `stopped` event reloads the current screen plus Home**, since browse rows are fetched once and go stale as episodes are watched. Rejected: the status as the trigger — `stop_playback` publishes it before the `Stopped` POST is answered, so a read then can get the old watched state.
- **Home always reloads** because Continue Watching and Next Up are what finishing invalidates. The screen half is the `reload_current_screen` that `r` calls, so the two cannot drift; a successful `t` calls `reload_screen_and_home` too.
- **The Playing screen fetches its item and season on a new item id, and again on `reported`**: the first fills the screen, the second corrects the watched marks. The duplicate pair per handover is deliberate.

## Where a complaint goes

- **The bottom row is key bindings only.** `App::message` (a failed request, a play with no daemon) goes in the **header**, elided by `view::to_width` and retired by the next keypress.
- **The header holds the tabs (`Fill(1)`), the message, the update badge and the daemon dot.** The badge eats the message's budget, so it appears only when an update is offered, and it carries its key (`↑<version> u`) because no hint row has room.
- **Every row fits 80 columns, held by a test.** `render_hint` does not wrap, so an overrun silently cuts the last bindings (usually `q quit`). Hence arrows rather than `h`/`j`/`k`/`l`, no `/` (Search is a tab), and `g`/`G` only in the log pane, since no arrow reaches top or bottom.
- **The dot (`view::daemon_status`) is green while the control socket is connected, red while not, and grey until the first connect resolves.** A successful play is not announced; the footer's title is the confirmation.

## View modes

- **A level is a grid or a list with a detail rail, chosen by item kind, not `Source`** (`nav::mode`): `Series`, `Season`, `Movie`, `BoxSet`, libraries and `Episode` get the grid. By kind, a folder of movies gets the grid whichever route reached it, and it is one function to test rather than a table kept in step with the browse stack. Only the first row is asked, so kinds sharing a screen must answer alike: `CollectionFolder` and `UserView` both come from `/UserViews`, and a Libraries screen changing shape with its sort order would be the bug.
- **A level still loading is one panel with a spinner** (`body::render_pending`), whatever it will become: `nav::mode` needs a row. The loop wakes every `SPINNER_FRAME` until it fills or fails. Rejected: guessing the shape from `Source`, since a `Folder` can turn out a list or a grid.
- **A failed load lands on its level** (`Msg::LevelFailed`, matched like `Msg::Level`) while it has no rows: the panel says "could not load" and stops spinning until `r` loads it again, while the error goes to the header like any other. Matching has no generation, so an older request's failure can flag a newer load still in flight; its rows clear it on arrival, and `REQUEST_TIMEOUT` bounds how long that takes.
- **Search is always a list**, its rows being mixed kinds.
- **Home is two one-row grids**, Continue Watching above Next Up, which is why `grid::metrics` takes the row count. A shelf is always landscape (`grid::shelf_metrics_for`), since Continue Watching mixes movies and episodes, and `offset` scrolls it horizontally by screenfuls.
- **`grid::render` draws with the `Metrics` `App` computed**, so requested covers and drawn tiles cannot disagree. Home bypasses `App::grid_metrics`, which answers only for the focused shelf; `App::visible_covers` asks for both.
- **The rail splits the width in half (`rail::split`)**, since row text elides gracefully and the cover is worth the width; below `MIN_BODY_WIDTH` there is no rail.

### Keys

- **In a grid, `h`/`j`/`k`/`l` and the arrows move the cursor and `Esc` is the only way back.** Up and down move a whole row, so `App` stores the terminal size each iteration; on Home they switch shelves (`App::move_vertically`), each with its own cursor.
- **`keys.rs` is a pure mapping**; `App::apply` drops `Intent::Left`/`Right` outside a grid, so in a list `Esc` and `Enter` are the only ways back and in.

### Tiles, rail and synopsis band

- **The rule under each cover is the progress bar**; the caption carries what it cannot (`46 left · ★ 8.0`, no count when finished), and selection is the caption's highlight.
- **`rail::meta` splits on `kind()`**: a playable item shows its runtime, a series or season counts its children, because a series' `RunTimeTicks` is one episode's length. The year is `ProductionYear`; `Status` and `EndDate` describe season one. Genres only when sent.
- **`ITEM_FIELDS` requests `RecursiveItemCount`, and `seasons()` passes it**: a series' progress comes only from `UserData.PlayedPercentage`, which the server leaves null without it. `ChildCount` and `Genres` ride along for about 10 KB per hundred rows.
- **Every grid but Libraries sits over a fixed `synopsis::HEIGHT` band** (`Mode::Synopsis`) with the selected item's title, meta and clipped synopsis. `App::grid_metrics` sizes the grid from what `synopsis::split` leaves. Rejected: a band sized to the synopsis, which resizes the grid and re-requests every cover per cursor move.
- **The band's meta drops series and year**, which the title carries, for runtime, rating, `♥` and last-watched day; series, seasons and collections drop runtime too.
- **`view::overview` handles the basic HTML some providers write into overviews**, which the web client renders as markdown: `<br>`/`<p>` break, `<i>`/`<em>`/`<b>`/`<strong>` style, other tags keep their text, a stray `<` stays; whitespace follows markdown, and markdown syntax shows as written. Rejected: `tui-markdown`, which prints inline HTML, and `html2text`, a whole html5ever parser for a handful of tags.

### Tile sizing

- **Tiles are sized by height**: `grid::metrics` divides the body into `TARGET_ROWS` and derives the cover width, so a tall screen gets bigger covers rather than more rows. Its bounds:
  - no narrower than `minimum_cover_width` (16 cells for 2:3, 24 for 16:9), unless the area is too short for one row that wide;
  - never so wide that a row holds fewer than `MIN_COLUMNS`, which stops a still taking a third of a wide screen, unless the level has fewer items;
  - no height budget when even the minimum width cannot reach `TARGET_ROWS`, since capping a cover that could never fit two rows only shrinks the one row that does.
- **No budget is not no cap.** `render` skips a tile taller than its area, so every cover is held to `one_row_budget`. A one-row shelf takes that as its budget and drops the width floor, since height binds it and widening a tile only fits fewer of the same covers; a grid too short for one row at the floor drops it too.
- **`cover::fit` fits against both the evened-out width and the budget**, since evening the columns hands each a few extra cells and a poster would outgrow the height.
- **`Metrics::rows` is what is drawn, not what fits.** The budget always divides by `TARGET_ROWS`, so a short level never gets taller covers. Tiles start at the top: centring against undrawn rows leaves a gap above the only row.

## Artwork

- **`Picker::from_query_stdio()` must run before `view::enter()`**: it writes an escape and reads the answer from stdin, which the alternate screen swallows. On failure (tmux without passthrough, a plain xterm) `Picker::halfblocks()` is the fallback.
- **Images come through `Api::image` → `Api::get`**, carrying the cached auth header. `jellyfin::url::image_url` is for MPRIS only: tokenless (the endpoint is anonymous), JPEG at 600 px since WebP is unsafe for a third-party widget, and with a named quality (a 265 KB poster is 226 KB with only the cap, 75 KB with the quality).
- **The server resizes**, to the next 64 px bucket above the cell box, as WebP. `maxWidth`/`maxHeight` keep aspect and cap both sides; `fillWidth`/`fillHeight` return a full poster whatever the box. A rail cover is about 40 KB.
- **The bucket is for the server's cache**: `ImageProcessor` re-encodes on demand, keyed by dimensions, format and quality, so exact `cells × font size` would encode per resize. It does not cut our requests (`Covers::claim` is keyed on the exact size and cell); the bandwidth win is WebP's.
- **The final downscale is local, with `FilterType::Lanczos3`**; the default `Nearest` is visibly bad on small tiles. `format=Webp` is not guaranteed (the original comes back for unsupported sources and GIFs), so the JPEG decoder stays.
- **A cover's size is part of its identity.** A `Protocol` is encoded for one rect at one cell size, so `CoverKey` is `(item id, image type, image tag, size, cell)`. `visible_covers` is recomputed each iteration, so a resize needs no notice; the cell catches a scale change that keeps the grid.
- **`Primary` is not always a poster.** `cover::primary_aspect` uses the server's `PrimaryImageAspectRatio` when sent, else 16:9 for an episode still or library banner and 2:3 for a poster. The guess stays because `/Items` sends the ratio only when asked and an artless library sends none; a `CollectionFolder` guesses a banner. A wrong box shows as a gap, and a grid sized from its first row mis-shapes the whole screen.
- **The box picks the image (`cover::image_for`)**: a landscape box takes a landscape Primary, else `Thumb`, then the first `Backdrop`, then the poster. The rail and `3 Playing` cut the box to the item's own Primary; a grid to its first row's.

### HiDPI

- **The cell size is measured each iteration, not trusted from startup.** Kitty is given pixel sizes and divides by the real cell size, while `ratatui-image` lays out against the `CSI 16 t` answer from before the alternate screen, which cannot be re-queried. When they disagree covers draw at 1/scale in the box's top-left corner; dragging to another display does the same.
- **`cover::cell_size` divides `ws_xpixel`/`ws_ypixel` (`Backend::window_size`, from `TIOCGWINSZ`) by the grid**, and `Covers::set_cell_size` rebuilds the picker at that font size keeping the queried protocol. `Picker::from_fontsize` is deprecated and 11.x cannot update a returned picker.
- **It runs at the head of the loop, not in `Event::Resize`**, since a scale change need not resize the grid. A change drops the whole cache (keys carry the cell) and in-flight results land under the old key. A change within a few percent is window padding and ignored; a terminal reporting no pixel size keeps the query's answer.
- **Never encode a cover larger than its box**: `Image::render` draws nothing on overflow without `allow_clipping`. Scale belongs in the picker's font size.

### Fetching

- **Decode and encode run in `spawn_blocking`**, being real CPU work on the drawing thread; the `Protocol` returns over `Msg`, so covers need no `select!` arm.
- **Fetches are rate-limited to one batch per `COVER_THROTTLE`, leading edge**: the first change after a quiet spell goes out at once, later ones wait for the window, and the last change standing fires, so a resting cursor is always fetched. Rejected: a trailing debounce, which makes every keypress pay the window on top of a round trip of similar length. `Covers::claim` keeps a resting cursor or revisited row to one request.
- **The batch gate is `Covers::any_missing`, not "the wanted set changed"**; otherwise a cover evicted after the cursor stops (a drag-resize churns the `CACHE_CAPACITY` entries) stays lost.
- **A failed request is final.** With that gate a retryable failure would re-request every throttle window forever. `Covers::give_up` files failures with absent images in `unavailable`, answered for the session and cleared only by a scale change; eviction is unaffected. Both log at `debug`, for the `L` pane.

### Disk cache

- **Memory holds at most `CACHE_CAPACITY` `Protocol`s**, since each is a decoded frame and more is straight memory growth; they are resize- and scale-specific, so none persist.
- **Disk holds the server's bytes** (about 40 KB of WebP each) under `Paths::cover_cache_dir`, which `--config` moves too, turning a memory miss into a local read and decode. `cover/disk.rs` uses plain `std::fs` inside the existing `spawn_blocking`; `tokio::fs` would also cost the daemon a tokio feature.
- **The key is the bucket, not the box**: `{item_id}-{image_type}-{image_tag}-{bucket_w}x{bucket_h}.img` through core's `bucket_pixels`, the request's own rounding, or a window drag would store a file per pixel for one request. It is unhashed, so the id and tag are sanitised: a `/` or `..` would make the key a path.
- **Absence stays in memory** (`Covers::unavailable`); a negative disk entry would need its own invalidation.
- **Nothing on disk may fail a cover**: every path degrades to a miss, and an undecodable file (a half-write) is removed.
- **LRU by mtime under `cover_cache_mb`** (default 256, `0` off), and reads touch the file. A write prunes after a quarter of the budget has been written since the last prune, and `main.rs` prunes at startup so a lowered budget applies. Pruning goes to 90%, or a full cache would prune again on the next cover.

## The Playing screen

- **`3` shows what the daemon plays**: a banner with the episode still, title, meta, rating and synopsis, then the season with the playing episode accented and `Enter` to jump. Below `MIN_BANNER_WIDTH` the still is dropped (`playing::still_rect` → `None`).
- **The status lacks synopsis and season, so the screen fetches `/Items/{id}` and then its season**, keyed by item id and dropped if playback moved on; the daemon's `display_title` fills it until then.
- **The lookup is sent once per item id (`App::playing_requested`)**, or a failing or slow one repeats every second with a warning each time.

## The log pane

- **`L` opens it and `L` or Esc closes it; the tab strip names it only while open**, being a debugging screen.
- **`logs::install` builds the subscriber in `main` before the alternate screen**, so a bad filter is reported on the normal one. It is an `EnvFilter` plus one `Layer` into a bounded `VecDeque` with no `fmt` layer, so nothing reaches stdout. The filter is `core`'s `log_filter`, so `RUST_LOG` means what it does for the daemon and `jellysink_core=debug` adds the HTTP layer's events. The daemon's logs are not here: `control.sock` carries none.
- **Levels**: `info` is user actions with an effect plus each request's time and row count; `debug` adds keypresses; `trace` adds daemon frames and cover timings. Status frames are not `info` — once a second they would flush the 2000-line buffer within half an hour; the connected/disconnected transition is.
- **Scrolling is anchored by sequence number**, since the ring evicts from the front. `Ring` counts `first_seq` past dropped lines and `log_anchor` holds the top line's; reaching the bottom clears the anchor so the view follows again.

## Terminal ownership

- **mpv has all three stdio handles on `/dev/null`** and its own window.
- **`jellytui` never calls `init_tracing`**, whose `fmt` layer writes to stdout.
- **`view::enter` is `ratatui::try_init`**, whose panic hook restores the terminal.
- **Input is read on a detached OS thread**, since `crossterm::event::read` blocks; at exit it is parked in `read` and the process ends anyway.
