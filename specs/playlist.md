# Playlist behaviour

How jellysink builds the queue, fetches episode data, and hands it to mpv.
Two external constraints shape all of it: Jellyfin's episode listing is a
forward-only cursor, and mpv's `loadfile … replace` wipes the playlist. Both
are verified, not inferred — see [Verified external behaviour](#verified-external-behaviour).

## The structures

| Structure        | Lives in                                 | Meaning                                                                    |
| ---------------- | ---------------------------------------- | -------------------------------------------------------------------------- |
| `Queue`          | `crates/jellysink/src/runtime/window.rs` | The authoritative ordered list of item ids, plus `index` (the current one). |
| `PlaylistWindow` | `crates/jellysink/src/runtime/window.rs` | Owns the `Queue` **and** how much of it mpv holds. All the arithmetic below. |
| mpv's playlist   | the mpv process                          | What the user sees in the playlist selector.                                |
| `titles`         | `Runtime`                                | Item id → display title, filled from the series listing.                    |
| `prepared`       | `Runtime`                                | Item id → `PreparedPlay`, only for items that have actually started.        |

The queue is the source of truth and mpv's playlist is a *window* onto it.
`prepared` is populated when an item starts, never in advance, so we do not
`PlaybackInfo` a whole series just to paint names.

## The window invariant

mpv's playlist is always a contiguous slice of the queue:

```
mpv playlist == queue.items[origin .. origin + head + 1 + tail]
```

- `origin` — queue index at mpv playlist position 0.
- `head` — entries already in mpv *before* the current item.
- `tail` — entries already in mpv *after* it.
- The `+ 1` is the current item itself.

All four are private to `PlaylistWindow`. New access is a method on it, not a
reach past it.

Two things that are easy to get wrong:

1. **Prepending does not change `origin`.** Splicing n entries before the
   current item shifts its queue index *and* its mpv position by the same n,
   so only `head` grows.
2. **The current position is `queue.index - origin` (`expected_pos`), not
   `head`.** The two coincide immediately after a prepend and diverge as soon
   as `adopt_playlist_pos` moves `index` on a playlist jump. Deriving it from
   `head` misreads every subsequent EOF and kills autoplay.

## Lifecycle of a play

### 1. Jellyfin sends the initial queue

A `Play` command carries `ItemIds` and `StartIndex`. For a series episode
Jellyfin typically sends the current episode *and everything after it* —
casting episode 6 of 20 sends 6..20. `CastEvent::PlayNow` →
`queue.replace(item_ids, start_index)`.

### 2. `start_current` (`runtime/playback/mod.rs`)

Resets the window, clears the `prepared` and `titles` caches, then:

1. **Prepare the current item** — `prepare_item` → `fetch_prepared`. The only
   *blocking* fetch; playback cannot start without it.
2. **Expand the series** — `maybe_expand_series`, which also caches titles.
3. **Load into mpv** — `loadfile … replace`, which **wipes mpv's playlist**.
4. **Forward fill** — `fill_forward_into_mpv`, one `loadlist append`.
5. **Prepend fill** — `fill_previous_into_mpv`, one `loadlist insert-at 0`.

Steps 4 and 5 must follow the load, since `replace` would wipe them. Neither
does any HTTP.

### 3. Series expansion (`runtime/queue/expand.rs`)

`maybe_expand_series` fetches the **whole series** in one request and splits it
at the current item with `split_episode_ids`, which returns `(previous, remaining)`:

```
GET /Shows/{seriesId}/Episodes?userId=…&Limit=500
```

**Why the whole series and not a cursor.** `StartItemId` is implemented as
`SkipWhile(i => i.Id != X)`, so it can never return episodes *before* the
current one. `AdjacentTo` is `FilterForAdjacency`, which narrows the listing to
the item's season and so gives nothing from earlier seasons. Omitting
`StartItemId` is the only way to see backwards.

**The two directions have separate gates**, and merging them is the mistake to
avoid — a shared gate means the prepend never runs in the common case:

| Direction      | Gate                                                | Why                                                                |
| -------------- | --------------------------------------------------- | ------------------------------------------------------------------ |
| Forward append | `series_expand_skip_reason` — skips when `has_next` | Jellyfin already sent 6..20; appending again would duplicate 7..20. |
| Prepend        | `prepend_skip_reason` — **ignores** `has_next`      | Jellyfin sending 6..20 is exactly when we also want 1..5.           |

They also differ on `autoplay`: it governs continuing *forward*, not what the
playlist selector can reach.

**Idempotency.** `ids_missing_from` filters out ids already in the queue, so
advancing e6 → e7 (which leaves e1..e6 queued ahead of e7) re-expands to
nothing.

**Titles.** The listing is fetched for any episode, even when both queue gates
skip. `media::episode_titles` (`media/title.rs`) stores a `display_title` per
id, and that is what the selector shows.

### 4. Preparing an item (`fetch_prepared`)

Only when that item **actually starts** — `start_current` for the first file,
`adopt_playlist_pos` on a playlist jump or autoplay. Two requests issued
concurrently via `tokio::join!`:

- `POST /Items/{id}/PlaybackInfo` — required. The media source, the stream maps,
  and whether DirectPlay is possible.
- `GET /Items/{id}` — optional. Display title and `SeriesId` for expansion; a
  failure is logged and ignored.

If neither DirectPlay nor DirectStream is offered, preparation **fails** and
that item is refused: jellysink does not transcode. Playlist rows are not gated
this way — they are stub URLs.

### 5. Filling mpv

No per-item HTTP. Each direction is one M3U written next to the IPC socket:

- Forward: `queue.items[origin + head + 1 + tail ..]`, `loadlist append`.
- Prepend: `take_pending_prepend`, `loadlist insert-at 0`.
- PlayNext: the spliced ids, `loadlist insert-at expected_pos + 1`
  (`insert_next_into_mpv`), run immediately — unlike the prepend there is no
  later `loadfile … replace` to wait out.

Each entry is a display title plus a DirectPlay stub whose `MediaSourceId` is
the item id. That is enough for a normal episode; a stacked version is resolved
by `PlaybackInfo` when the user plays that row. With no title (PlayNext of a
non-series item, listing failed) the selector shows the URL — a **tokenless**
one, since the playable URL would put `ApiKey=` into mpv's OSD, the playlist
selector and the user's `watch_later` files.

`ApiKey=` appears on a row URL only when mpv is *not* carrying the
`Authorization` header (`Runtime::mpv_auth_header_set`, set by `apply_auth`).
The header is a global mpv property, so it covers rows loaded later too.

## Handing entries to mpv

### Why M3U temp files

```m3u
#EXTM3U
#EXTINF:-1,Show - s1e01 - Pilot
http://server/Videos/{id}/stream?static=true&MediaSourceId={id}
```

`loadfile`'s `force-media-title` and the `playlist/N/title` property **do not
populate unloaded entries**, so without the M3U every row in the selector shows
a raw URL. The file is written mode `0600` and removed right after the load.

### The three commands

| Command    | Args                         | Effect                                    |
| ---------- | ---------------------------- | ----------------------------------------- |
| `loadfile` | `[url, "replace"]`           | Plays now. **Wipes the entire playlist.** |
| `loadlist` | `[path, "append"]`           | Adds to the end.                          |
| `loadlist` | `[path, "insert-at", index]` | Splices in at `index`.                    |

`insert-at` and the index must be **separate arguments**; `"insert-at0"` is
`invalid parameter`. The flag arrived in **mpv 0.38**, which is therefore
jellysink's minimum; the two integration tests exercising it are gated on
`require_mpv!(0, 38)` rather than failing on a distro-pinned box.

Inserting at or below the current position does **not** interrupt playback —
mpv shifts `playlist-pos` by the number inserted and keeps playing the same
file. That is what makes prepending viable at all.

### Ordering constraint

`maybe_expand_series` runs *before* the load (it needs the item metadata, and
the load needs the prepared URL), but `loadfile … replace` wipes the playlist,
so the prepend cannot run before the current file is loaded. Hence the split:

- `prepend_previous_episodes` → `PlaylistWindow::prepend` — queue bookkeeping
  only, during expansion.
- `fill_previous_into_mpv` — the mpv insertion, after the load, draining
  `PlaylistWindow::take_pending_prepend`.

Doing both in one step guarded on `self.mpv.is_some()` silently never fires on
first play, because mpv has not spawned yet.

## Navigation and EOF

### `keep-open=yes`

`yes` pauses only on the *last* playlist entry and auto-plays the rest, which
is what emits the `end-file` the runtime adopts the new item from. `always`
pauses on the last frame of every file without unloading it, so `end-file`
never fires and autoplay stalls.

### `playlist_eof` (`runtime/window.rs`)

| Case                          | Result                                 |
| ----------------------------- | -------------------------------------- |
| `pos > expected_pos`          | `WaitForMpv` — mpv already advanced.   |
| `pos + 1 < count`, from EOF   | `WaitForMpv` — let mpv autoplay.       |
| `pos + 1 < count`, user Next  | `NextInMpv` — `playlist-next`.         |
| No next in mpv, queue has one | `NextNotInMpv` — advance the queue.    |
| Nothing left                  | `Stop` (after trying a series expand). |

The `WaitForMpv` cases exist because with `keep-open=yes` mpv has often already
moved to the next entry by the time `end-file` arrives; issuing `playlist-next`
on top of that skips to N+2.

### Playlist jumps

`adopt_playlist_pos` runs on `FileLoaded` and maps mpv's `playlist-pos` back to
a queue index via `queue_index_at`. If it differs from the current item, the
runtime sends Stopped, adopts the new index, re-prepares if needed, and sends
Start. **This is why the playlist selector works for free** — no dedicated jump
handling exists.

`CastEvent::Previous` uses `playlist-prev` when `playlist-pos > 0` and only
falls back to `queue.previous()` plus a restart at position 0. With prepending
on, `playlist-pos` is rarely 0, so Previous reaches back across the series
rather than dead-ending at the queue start.

## Reporting back to Jellyfin

`NowPlayingQueue` (`report.rs`) sends the **entire** `queue.items` with
`PlaylistItemId: playlistItem{i}`, so with prepending the now-playing view
shows 1..20 rather than 6..20. The payload is an `Arc` shared from
`PlaylistWindow`, not rebuilt per report — a progress report goes out once a
second and carries the whole queue.

## Configuration

| Key                | Default | Effect                                                   |
| ------------------ | ------- | -------------------------------------------------------- |
| `autoplay`         | `true`  | Continue forward into the next episode.                  |
| `prepend_previous` | `true`  | Also load earlier episodes so the selector reaches them. |

## Known limits

- **Long series.** `episodes_all` caps at 500. Past that the current item is not
  in the listing, `split_episode_ids` returns empty on both sides, and expansion
  fails closed. Paging is not implemented.
- **Stub `MediaSourceId`.** Playlist rows use the item id; multi-version items
  are corrected when that row actually starts.
- **Specials.** An item absent from the listing (specials, library churn,
  alternate versions) skips queue expansion. Titles are still cached.

## Verified external behaviour

Checked live against mpv 0.41.0 and the Jellyfin server source:

- `loadlist … insert-at <index>` inserts in order and does not interrupt
  playback; `playlist-pos` shifts by the number inserted.
- It works with plain HTTP URLs and without `--load-unsafe-playlists`, which
  jellysink does not pass.
- `loadfile … replace` reduces a 3-entry playlist to 1.
- `StartItemId` is `SkipWhile` (forward-only); `AdjacentTo` is
  `FilterForAdjacency` (season-scoped).

## Where this is tested

The window arithmetic is the risky part: `runtime/window_test.rs`, driving
`PlaylistWindow` itself rather than a reimplementation of it. The expansion
helpers and the M3U/stub builders are covered beside their own files
(`runtime/queue/expand_test.rs`, `runtime/queue/stubs_test.rs`,
`mpv/command_test.rs`); mpv's own behaviour above is pinned in
`mpv/integration_test.rs`.
