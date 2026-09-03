# Playlist behaviour

How jellysink builds the queue, fetches episode data, and hands it to mpv.

This document describes the system as it stands, including *why* the awkward
parts are the way they are. Most of the constraints here come from two places:
Jellyfin's API being a forward-only cursor, and mpv's `loadfile` wiping the
playlist. Both are verified behaviours, not guesses — see
[Verified external behaviour](#verified-external-behaviour).

## The three structures

There are three parallel notions of "what is playing next", and keeping them
in sync is most of the complexity.

| Structure        | Lives in                    | Meaning                                                                     |
| ---------------- | --------------------------- | --------------------------------------------------------------------------- |
| `Queue`          | `src/runtime/window.rs`     | The authoritative ordered list of item ids, plus `index` (the current one). |
| `PlaylistWindow` | `src/runtime/window.rs`     | Owns the `Queue` **and** how much of it mpv holds. All the arithmetic below. |
| mpv's playlist   | the mpv process             | What the user actually sees in the playlist selector.                       |
| `titles`         | `Runtime`                   | Item id → display title, filled from the series listing.                    |
| `prepared`       | `Runtime`                   | Cache of item id → `PreparedPlay` for items that have actually started.     |

The queue is the source of truth. mpv's playlist is a *window* onto it.
`titles` is what the selector shows. `prepared` is only populated when an
item starts (first play or a jump / autoplay), so we never `PlaybackInfo`
the rest of the series just to paint names.

### The window invariant

mpv's playlist is always a contiguous slice of the queue:

```
mpv playlist == queue.items[origin .. origin + head + 1 + tail]
```

- `origin` — queue index at mpv playlist position 0.
- `head` — entries already in mpv *before* the current item (previous episodes
  spliced in).
- `tail` — entries already in mpv *after* the current item.
- The `+ 1` is the current item itself.

The current item's mpv position is therefore `queue.index - origin`
(`PlaylistWindow::expected_pos`).

All four live inside `PlaylistWindow`, private to it, reachable only through
`expected_pos`, `queue_index_at`, `forward_ids`, `note_appended`, `prepend`,
`take_pending_prepend`, `reset_to_current` and `clear`. They used to be five
loose fields on `Runtime` maintained by hand at six call sites.

Two consequences that are easy to get wrong:

1. **Prepending does not change `origin`.** Splicing n entries before the
   current item shifts its queue index *and* its mpv position by the same n,
   so `origin` is unchanged and only `head` grows. (An earlier version did
   `origin += n` and was wrong.)
2. **The current position is `queue.index - origin`, not `head`.** They
   coincide immediately after a prepend, but diverge as soon as
   `adopt_playlist_pos` moves `index` on a playlist jump. Deriving it from
   `head` misreads every subsequent EOF and breaks autoplay. This was a real
   bug; `expected_pos_follows_a_playlist_jump_not_the_head` pins it.

## Lifecycle of a play

### 1. Jellyfin sends the initial queue

A `Play` command over the WebSocket carries `ItemIds` and `StartIndex`. For a
series episode, Jellyfin typically sends the current episode *and everything
after it* — casting episode 6 of 20 sends 6..20 (15 items).

`CastEvent::PlayNow` → `queue.replace(item_ids, start_index)`.

### 2. `start_current` (`src/runtime/playback.rs`)

Resets the window (`PlaylistWindow::reset_to_current`), clears the `prepared`
and `titles` caches, then:

1. **Prepare the current item** — `prepare_item` → `fetch_prepared`. This is
   the only *blocking* fetch; playback cannot start without it.
2. **Expand the series** — `maybe_expand_series` (below). Also caches titles
   from the listing for every episode.
3. **Load into mpv** — `loadfile ... replace`, which **wipes mpv's playlist**.
4. **Forward fill** — `fill_forward_into_mpv`, one `loadlist append`.
5. **Prepend fill** — `fill_previous_into_mpv`, one `loadlist insert-at 0`.

Steps 4 and 5 run after the load because `loadfile ... replace` would wipe
anything already in mpv. They do no HTTP.

### 3. Series expansion (`maybe_expand_series`)

Fetches the **whole series** in one request and splits it at the current item:

```
GET /Shows/{seriesId}/Episodes?userId=…&Limit=500
```

`split_episode_ids` (`src/runtime/queue.rs`) returns `(previous, remaining)`.

**Why the whole series and not a cursor.** `StartItemId` is implemented as
`SkipWhile(i => i.Id != X)` — a forward-only cursor. It can never return
episodes *before* the current one. `AdjacentTo` is not a neighbour query
either; it is `FilterForAdjacency`, which narrows the listing to the item's
season, so it gives nothing from earlier seasons. Omitting `StartItemId` is
the only way to see backwards.

**The two directions have separate gates.** This is the subtlest part:

| Direction      | Gate                                                | Why                                                                 |
| -------------- | --------------------------------------------------- | ------------------------------------------------------------------- |
| Forward append | `series_expand_skip_reason` — skips when `has_next` | Jellyfin already sent 6..20; appending again would duplicate 7..20. |
| Prepend        | `prepend_skip_reason` — **ignores** `has_next`      | Jellyfin sending 6..20 is exactly when we also want 1..5.           |

Sharing one gate meant the prepend never ran in the common case — casting
episode 6 produced only 6..20. The gates also differ on `autoplay`: it governs
continuing *forward*, not what the playlist selector can reach.

**Idempotency.** `ids_missing_from` (`src/runtime/queue.rs`) filters out ids already in the queue, so
advancing e6 → e7 (which leaves e1..e6 already queued ahead of e7) adds
nothing on re-expansion.

**Titles.** The listing is fetched for any episode, even when both queue
gates skip (Jellyfin already sent 6..20, prepend off, …).
`media::episode_titles` (`src/media/title.rs`) walks `Items` and stores `display_title` for every
id. That is what the selector shows.

### 4. Preparing an item (`fetch_prepared`)

Only when that item **actually starts**: `start_current` for the first file,
`adopt_playlist_pos` on a playlist jump or autoplay. Two requests issued
**concurrently** via `tokio::join!`:

- `POST /Items/{id}/PlaybackInfo` — required. Yields the media source, stream
  maps, and whether DirectPlay is possible.
- `GET /Items/{id}` — optional. Display title and series metadata (`SeriesId`)
  for expansion; a failure is logged and ignored.

If neither DirectPlay nor DirectStream is supported, preparation **fails** and
playback of *that* item is refused. This is deliberate: jellysink does not
transcode. Playlist rows are not gated this way — they are stub URLs.

### 5. Filling mpv — titles from the listing, one loadlist per side

No per-item HTTP. Each direction is one M3U written next to the IPC socket:

- Forward: remaining `queue.items[origin + head + 1 + tail ..]`, `loadlist
  append`.
- Prepend: `pending_prepend`, `loadlist insert-at 0`.

Each entry is a display title plus a DirectPlay stub:

```
/Videos/{id}/stream?static=true&MediaSourceId={id}&ApiKey=…
```

`MediaSourceId` is the item id. That is enough for a normal episode; stacked
versions are resolved later by `PlaybackInfo` when the user actually plays
that row. If a title is missing (PlayNext of a non-series item, listing
failed), the selector shows the URL — but a **tokenless** one. The fallback
used to be the playable URL, which put `ApiKey=` into mpv's OSD, the playlist
selector, and the user's `watch_later` files.

`ApiKey=` is on the row URL only when mpv is *not* carrying the `Authorization`
header (`Runtime::mpv_auth_header_set`, set by `apply_auth`). The header is a
global mpv property, so it covers rows loaded later too; leaving the token off
keeps it out of anything mpv persists.

## Handing entries to mpv

### Why M3U temp files

Each direction is one M3U next to the IPC socket, loaded, then deleted:

```m3u
#EXTM3U
#EXTINF:-1,Show - s1e01 - Pilot
http://server/Videos/{id}/stream?static=true&MediaSourceId={id}
#EXTINF:-1,Show - s1e02 - Name
http://server/Videos/{id}/stream?static=true&MediaSourceId={id}
```

(`&ApiKey=…` is appended to each URL only when the `Authorization` header is
not in play — see above.)

This exists because `loadfile`'s `force-media-title` and the
`playlist/N/title` property **do not populate unloaded entries** — without the
M3U, every entry in the selector shows a raw URL instead of a title. The temp
file is written mode `0600` and removed immediately after the load.

### The three commands

| Command    | Args                         | Effect                                    |
| ---------- | ---------------------------- | ----------------------------------------- |
| `loadfile` | `[url, "replace"]`           | Plays now. **Wipes the entire playlist.** |
| `loadlist` | `[path, "append"]`           | Adds to the end.                          |
| `loadlist` | `[path, "insert-at", index]` | Splices in at `index`.                    |

`insert-at` and the index must be **separate arguments**; `"insert-at0"` as a
single token is `invalid parameter`.

Inserting at or below the current position does **not** interrupt playback —
mpv shifts `playlist-pos` by the number inserted and keeps playing the same
file. This is what makes prepending viable at all.

### Ordering constraint

Because `loadfile ... replace` wipes the playlist, the prepend **cannot** run
before the current file is loaded. But `maybe_expand_series` runs *before* the
load (it needs the item metadata, and the load needs the prepared URL). The
split is:

- `prepend_previous_episodes` → `PlaylistWindow::prepend` — queue bookkeeping
  only, runs during expansion.
- `fill_previous_into_mpv` — mpv insertion, runs after the load, from
  `PlaylistWindow::take_pending_prepend`.

An earlier version did both in one step guarded by `self.mpv.is_some()`, which
silently never fired on first play because mpv had not spawned yet.

## Navigation and EOF

### `keep-open=yes`

`yes` pauses only on the *last* playlist entry and auto-plays the rest, which
is what emits `end-file` so the runtime can adopt the new item. `always`
pauses on the last frame of every file without unloading it, so `end-file`
never fires and autoplay stalls.

### `playlist_eof` (`src/runtime/window.rs`)

Decides what to do at end-of-file. `expected_pos` is `queue.index - origin`.

| Case                          | Result                                 |
| ----------------------------- | -------------------------------------- |
| `pos > expected_pos`          | `WaitForMpv` — mpv already advanced.   |
| `pos + 1 < count`, from EOF   | `WaitForMpv` — let mpv autoplay.       |
| `pos + 1 < count`, user Next  | `NextInMpv` — `playlist-next`.         |
| No next in mpv, queue has one | `NextNotInMpv` — advance the queue.    |
| Nothing left                  | `Stop` (after trying a series expand). |

The `WaitForMpv` cases exist because with `keep-open=yes` mpv has often
already moved to the next entry by the time `end-file` arrives; issuing
`playlist-next` on top of that would skip to N+2.

### Playlist jumps

`adopt_playlist_pos` runs on `FileLoaded` and maps mpv's `playlist-pos` back
to a queue index via `queue_index_at(origin, pos, len)`. If it differs from
the current item, the runtime sends Stopped, adopts the new index, re-prepares
if needed, and sends Start. **This is why the playlist selector works for
free** once entries are in mpv — no dedicated jump handling is needed.

`CastEvent::Previous` uses `playlist-prev` when `playlist-pos > 0` and only
falls back to `queue.previous()` + restart at position 0. With prepending,
`playlist-pos` is rarely 0, so Previous now works across the whole series
instead of dead-ending at the queue start.

## Reporting back to Jellyfin

`NowPlayingQueue` (`src/report.rs`) sends the **entire** `queue.items` with
`PlaylistItemId: playlistItem{i}`. With prepending enabled this reports 1..20
rather than 6..20, so Jellyfin's now-playing view shows the full series. This
is a deliberate, visible behaviour change.

## Configuration

| Key                | Default | Effect                                                   |
| ------------------ | ------- | -------------------------------------------------------- |
| `autoplay`         | `true`  | Continue forward into the next episode.                  |
| `prepend_previous` | `true`  | Also load earlier episodes so the selector reaches them. |

`prepend_previous false` restores the old forward-only behaviour without a
rebuild.

## Known limits

- **Long series.** `episodes_all` caps at 500. Past that the current item is
  not in the listing, `split_episode_ids` returns empty on both sides, and
  expansion fails closed — same as the pre-existing forward path. Paging is
  not implemented.
- **Stub `MediaSourceId`.** Playlist rows use the item id as `MediaSourceId`.
  Multi-version items are corrected when that row actually starts.
- **Specials.** If the current item is not in the listing (specials, library
  churn, alternate versions), queue expansion is skipped. Titles from the
  listing are still cached.

## Verified external behaviour

Checked live against mpv 0.41.0 and the Jellyfin server source, not inferred:

- `loadlist … insert-at <index>` inserts in order and does not interrupt
  playback; `playlist-pos` shifts by the number inserted.
- Works with plain HTTP URLs and without `--load-unsafe-playlists`, which
  jellysink does not pass.
- `loadfile … replace` reduces a 3-entry playlist to 1.
- `StartItemId` is `SkipWhile` (forward-only); `AdjacentTo` is
  `FilterForAdjacency` (season-scoped).

## Tests

The window arithmetic is the risky part and is covered in
`src/runtime/window.rs`, **against `PlaylistWindow` itself**. The tests used to
run against a `Window` struct in the test module that reimplemented the
arithmetic, so they could pass while the real code drifted:

- `prepend_keeps_the_window_contiguous_and_current_stable`
- `prepend_then_fill_appends_after_the_current_item`
- `prepend_from_a_mid_series_start_index`
- `prepend_happens_even_though_jellyfin_sent_a_full_queue`
- `advancing_then_prepending_does_not_duplicate`
- `expected_pos_follows_a_playlist_jump_not_the_head`
- `eof_expected_pos_is_not_zero_when_previous_episodes_are_loaded`

Plus unit tests for `playlist_stub_entry` (the token must never reach a row's
display title), `split_episode_ids`, `episode_titles`, `ids_missing_from`,
`prepend_skip_reason`, `Queue::insert_before_current`, `playlist_m3u`, and
`loadlist_insert_at_args`.
