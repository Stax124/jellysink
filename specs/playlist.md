# Playlist behaviour

How jellysink builds the queue, fetches episode data and hands it to mpv. Two verified external constraints shape it: Jellyfin's episode listing is a forward-only cursor, and mpv's `loadfile … replace` wipes the playlist.

## Structures

| Structure        | Lives in                                 | Meaning                                                      |
| ---------------- | ---------------------------------------- | ------------------------------------------------------------ |
| `Queue`          | `crates/jellysink/src/runtime/window.rs` | The ordered item ids, plus `index` (the current one).        |
| `PlaylistWindow` | `crates/jellysink/src/runtime/window.rs` | Owns the `Queue` and how much of it mpv holds.               |
| mpv's playlist   | the mpv process                          | What the user sees in the playlist selector.                 |
| `titles`         | `Runtime`                                | Item id → display title, filled from the series listing.     |
| `prepared`       | `Runtime`                                | Item id → `PreparedPlay`, only for items that have started.  |

- **The queue is the source of truth; mpv's playlist is a window onto it.**
- **`prepared` fills only when an item starts**, so a whole series is never `PlaybackInfo`'d just to paint names.

## The window invariant

```
mpv playlist == queue.items[origin .. origin + head + 1 + tail]
```

`origin` is the queue index at mpv position 0; `head` and `tail` are the entries mpv holds before and after the current item. All are private to `PlaylistWindow`; new access is a method on it.

- **Prepending does not change `origin`.** It shifts the current item's queue index and mpv position by the same n, so only `head` grows.
- **The current position is `expected_pos` (`queue.index - origin`), not `head`.** They diverge once `adopt_playlist_pos` moves `index`; deriving it from `head` misreads every later EOF and kills autoplay.

## Starting a play

A `Play` carries `ItemIds` and `StartIndex`; for an episode Jellyfin usually sends it and everything after (casting e6 of 20 sends 6..20). `CastEvent::PlayNow` → `queue.replace`.

`start_current` (`runtime/playback/mod.rs`) resets the window and the `prepared`/`titles` caches, then:

1. `prepare_item` → `fetch_prepared` — the only blocking fetch.
2. `maybe_expand_series`, which also caches titles.
3. `loadfile … replace`.
4. `fill_forward_into_mpv` — one `loadlist append`.
5. `fill_previous_into_mpv` — one `loadlist insert-at 0`.

- **The fills come after the load**, since `replace` wipes them; neither does HTTP.
- **Expansion runs before the load but only does queue bookkeeping** (`prepend_missing` → `PlaylistWindow::prepend`): it needs the item metadata, the load needs the prepared URL, and `replace` would wipe an earlier insertion. The mpv insertion drains `take_pending_prepend` after the load. Doing both in one step guarded on `self.mpv.is_some()` never fires on first play, because mpv has not spawned yet.

## Series expansion (`runtime/queue/expand.rs`)

One request for the whole series, `GET /Shows/{seriesId}/Episodes?userId=…&Limit=500`, split at the current item by `split_episode_ids` into `(previous, remaining)`, or `None` when it is not listed.

- **Rejected: a cursor.** `StartItemId` is `SkipWhile(i => i.Id != X)` and never returns earlier episodes; `AdjacentTo` (`FilterForAdjacency`) is season-scoped.
- **Only an `Episode` with a `SeriesId` expands, and the two directions have separate gates** (`expansion_directions`). A shared gate means the prepend never runs in the common case:

  | Direction      | Gate                                     | Why                                                                  |
  | -------------- | ---------------------------------------- | -------------------------------------------------------------------- |
  | Forward append | `autoplay` and not `has_next`            | Jellyfin already sent 6..20; appending again would duplicate 7..20.  |
  | Prepend        | `prepend_previous`, ignoring `has_next`  | Jellyfin sending 6..20 is exactly when we also want 1..5.            |

  `autoplay` governs continuing forward, not what the selector can reach.
- **Expansion is idempotent.** `ids_missing_from` drops ids already queued, so advancing e6 → e7 re-expands to nothing.
- **Titles are cached for any episode**, even when both gates skip; the selector shows `media::episode_titles`' `display_title`.

## Preparing an item (`fetch_prepared`)

Runs only when the item actually starts (`start_current`, or `adopt_playlist_pos` on a jump or autoplay), as two concurrent requests:

- `POST /Items/{id}/PlaybackInfo` — required: the media source, stream maps and whether DirectPlay is possible.
- `GET /Items/{id}` — optional: title and `SeriesId`; a failure is logged and ignored.

- **No DirectPlay or DirectStream means the item is refused**; jellysink does not transcode. Playlist rows are stubs and not gated.

## Filling mpv

No per-item HTTP; each direction is one M3U written next to the IPC socket:

- Forward: `queue.items[origin + head + 1 + tail ..]`, `loadlist append`.
- Prepend: `take_pending_prepend`, `loadlist insert-at 0`.
- PlayNext: the spliced ids at `insert-at expected_pos + 1`, at once, since no later `replace` follows.

```m3u
#EXTM3U
#EXTINF:-1,Show - s1e01 - Pilot
http://server/Videos/{id}/stream?static=true&MediaSourceId={id}
```

- **M3U because `force-media-title` and `playlist/N/title` do not populate unloaded entries**; without it every row shows a URL. Written 0600 and removed after the load.
- **A row is a title plus a DirectPlay stub whose `MediaSourceId` is the item id.** A stacked version is resolved by `PlaybackInfo` when that row plays.
- **A row with no title shows a tokenless URL**, since `ApiKey=` would reach mpv's OSD, the selector and `watch_later`.
- **`ApiKey=` is on row URLs only when mpv is not carrying the `Authorization` header** (`Runtime::mpv_auth_header_set`, set by `apply_auth`). The header is global, so it covers rows loaded later, but it reaches every host and an autoplayed row skips `apply_auth`'s foreign-subtitle-host check; it is therefore set only when the queue has no next item. The accepted cost: with a next item queued, `ApiKey=` sits in playable row URLs and so in `watch_later`.

## mpv commands

| Command    | Args                         | Effect                                    |
| ---------- | ---------------------------- | ----------------------------------------- |
| `loadfile` | `[url, "replace"]`           | Plays now. **Wipes the entire playlist.** |
| `loadlist` | `[path, "append"]`           | Adds to the end.                          |
| `loadlist` | `[path, "insert-at", index]` | Splices in at `index`.                    |

- **`insert-at` and the index are separate arguments**; `"insert-at0"` is `invalid parameter`. It needs mpv 0.38, jellysink's minimum, and its integration tests are gated on `require_mpv!(0, 38)`.
- **Inserting at or below the current position does not interrupt playback**; `playlist-pos` shifts by the count inserted. Prepending depends on it.

## Navigation and EOF

- **`keep-open=yes`, not `always`.** `yes` pauses only on the last entry and autoplays the rest, emitting the `end-file` the runtime adopts from; `always` never unloads a file, so `end-file` never fires.

`playlist_eof` (`runtime/window.rs`):

| Case                          | Result                                 |
| ----------------------------- | -------------------------------------- |
| `pos > expected_pos`          | `WaitForMpv` — mpv already advanced.   |
| `pos + 1 < count`, from EOF   | `WaitForMpv` — let mpv autoplay.       |
| `pos + 1 < count`, user Next  | `NextInMpv` — `playlist-next`.         |
| No next in mpv, queue has one | `NextNotInMpv` — advance the queue.    |
| Nothing left                  | `Stop` (after trying a series expand). |

- **`WaitForMpv` exists because mpv has often advanced before `end-file` arrives**; a `playlist-next` on top skips to N+2.
- **Playlist jumps need no dedicated handling.** `adopt_playlist_pos` runs on `FileLoaded`, maps `playlist-pos` back through `queue_index_at`, and on a change prepares the new item, sends Stopped, adopts the index and sends Start. A failed prepare stops playback: keeping the old item would apply its stream maps to the new file.
- **`CastEvent::Previous` uses `playlist-prev` when `playlist-pos > 0`**, falling back to `queue.previous()` and a restart at 0; with prepending it reaches back across the series.
- **`NowPlayingQueue` (`report.rs`) sends the whole queue** with `PlaylistItemId: playlistItem{i}`, so Jellyfin shows 1..20, not 6..20.

## Configuration

| Key                | Default | Effect                                                   |
| ------------------ | ------- | -------------------------------------------------------- |
| `autoplay`         | `true`  | Continue forward into the next episode.                  |
| `prepend_previous` | `true`  | Also load earlier episodes so the selector reaches them. |

## Known limits

- **Series over 500 episodes.** `episodes_all` caps there, the current item may be missing, `split_episode_ids` returns `None` and expansion fails closed. No paging.
- **The header can still reach a foreign subtitle host.** The has-next check runs once, at load; rows prepended ahead of a last episode, or added later by PlayNext/PlayLast, can autoplay under it.
- **Stub `MediaSourceId`.** Multi-version items are corrected when that row starts.
- **Items absent from the listing** (specials, library churn, alternate versions) skip expansion; titles are still cached.

## Verified external behaviour

Checked against mpv 0.41.0 and the Jellyfin source:

- `loadlist … insert-at` works with plain HTTP URLs and without `--load-unsafe-playlists`.
- `loadfile … replace` reduces a 3-entry playlist to 1.
- `http-header-fields` set before a `loadlist` is sent when mpv advances into its entries, and to a `sub-add` host.
- `StartItemId` is `SkipWhile`; `AdjacentTo` is `FilterForAdjacency`.

Tests drive `PlaylistWindow` itself, never a reimplementation of its arithmetic.
