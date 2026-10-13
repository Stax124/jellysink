# Audio and subtitle tracks

How Jellyfin stream indexes map to mpv tracks, and how a picked track survives into the next episode. Two problems shape it: the two numberings only sometimes agree, and the server's `DefaultAudioStreamIndex`/`DefaultSubtitleStreamIndex` is what is being worked around — it picks the wrong track on mislabeled releases and the wrong half of a `Signs and Songs`/`Dialogue` split.

## Numbering

| Number                  | Whose  | Scope                                                                |
| ----------------------- | ------ | -------------------------------------------------------------------- |
| Jellyfin stream index   | server | One sequence over *all* streams of an item — video, audio, subtitle. |
| mpv `aid`               | mpv    | 1-based, per type, **muxed audio only**, in file order.              |
| mpv `sid` (embedded)    | mpv    | 1-based, per type, in file order.                                    |
| mpv `sid` (`sub-add`ed) | mpv    | Assigned on load, above the embedded range.                          |

- **Everything outside mpv speaks Jellyfin stream indexes** (`PlayRequest`, `PlayingState`, `CastEvent::SetAudio`/`SetSubtitle`). `StreamMaps` (`media/streams.rs`) is the whole translation layer:

  | Field                               | Direction                                   |
  | ----------------------------------- | ------------------------------------------- |
  | `audio_track_id_by_stream_index`    | Jellyfin index → `aid`                      |
  | `subtitle_track_id_by_stream_index` | Jellyfin index → embedded `sid`             |
  | `subtitle_url`                      | Jellyfin index → absolute URL to `sub-add`  |
  | `audios` / `subtitles`              | every stream mpv can actually be pointed at |

- **The reverse lookup (`Runtime::jellyfin_index_of`) is a linear scan**; a second map kept in sync is not worth it for one read per track change.
- **`sub-add`ed ids live in `Runtime::external_subtitle_track_ids`**, rebuilt per file, since they exist only once mpv has loaded it.

## Building the maps (`map_streams`)

- **External audio is skipped entirely.** mpv has no track for it, and mapping it hands back the next embedded `aid`. It stays out of `audios` too, so it can never be remembered.
- **Subtitles have two independent axes.** `DeliveryMethod` decides where the entry goes (in-file track or URL); `IsExternal` decides whether the mpv `sid` counter advances. Jellyfin reports an in-file subtitle it extracts to a sidecar as `External` while mpv still has a track for it, so gating the counter on delivery puts every later subtitle one off.
- **`selectable` gates entry into `subtitles`.** Its two `warn` arms (an `External` stream with no `DeliveryUrl`, an unknown delivery method) cannot be pointed at, so must never become a remembered choice that silently falls back every later episode.

## Remembering a choice

- **A choice is an identity (`TrackId`, `media/track.rs`), not an index**, re-matched against each item; indexes are per-file.
- **One `TrackId` serves both kinds**; only the `Runtime` field keeps them apart. `is_forced`/`is_external` are always `false` for audio, so they add a constant and cannot change an audio ranking.
- **`named()` treats `""`, `"und"` and `"undefined"` as absent**, since Jellyfin sends them rather than omitting the field. `same()` needs both sides present.

### Scoring

`score` gates on language, title or display title and returns `None` otherwise; flags, codec and index only rank, since alone they match unrelated streams (every non-forced embedded SRT agrees).

| Weight                   | Value | Meaning                                               |
| ------------------------ | ----- | ----------------------------------------------------- |
| `LANGUAGE`               | 1000  | Both name the same language.                          |
| `TITLE`                  | 400   | Same muxer track name (`Signs and Songs`).            |
| `DISPLAY_TITLE`          | 200   | Same Jellyfin display title.                          |
| `BOTH_LANGUAGES_UNKNOWN` | 200   | Neither names a language. Never qualifies on its own. |
| `FORCED`                 | 40    | `IsForced` agrees.                                    |
| `EXTERNAL`               | 20    | `IsExternal` agrees.                                  |
| `CODEC`                  | 10    | Same codec.                                           |
| `INDEX`                  | 5     | Tiebreak only.                                        |

- **The weights do not overlap, so the sum is lexicographic**: everything below `LANGUAGE` totals 875. A wrong language is worse than a wrong track in the right one.
- **`best_match` breaks ties on the lowest index**, so it resolves the same way every time.

### What cannot be remembered

- **`TrackPreference::from_selection` forgets the previous choice** for an index not in `candidates` or a stream with neither language nor name (`is_identifiable`). Keeping it would silently re-apply a track the user moved away from.
- **`stream_index < 0` is Off without consulting candidates**, a decision even for an item with no such streams.

## Applying it

`resolve_track_index` (`media/track.rs`), highest first:

| Rank | Source                    | Why                                                                           |
| ---- | ------------------------- | ----------------------------------------------------------------------------- |
| 1    | `requested`               | The remote named a stream for *this* item; nothing remembered outranks that.  |
| 2    | The remembered preference | Only when this item has a matching track.                                     |
| 3    | `server_default`          | The fallback.                                                                 |

- **`TrackPreference::Off` resolves to `-1`, an explicit no.** `sub-add` selects what it adds, so an unset `sid` shows the last external subtitle; audio Off is reachable through mpv's `cycle audio`.
- **`prepare_item` (`runtime/queue/mod.rs`) is the only producer of a `PreparedPlay`, so the only place memory is applied** (`with_remembered_tracks`). That is how it reaches playlist jumps and mpv's autoplay.
- **`prepared` caches the server's answer, not the overridden clone**, so a fallback means "what the server said", not "the preference when this episode first played".

`configure_streams` runs on `file-loaded`, in this order:

1. `sub-add` every external URL, recording each new `sid` from `max_subtitle_track_id`. Each `sub-add` selects its track, so no selection before step 3 is trustworthy.
2. Apply the resolved audio index.
3. Apply the resolved subtitle index.
4. Read `sid` and `aid` back and settle them; with nothing to apply, the selection is mpv's own.

## Picks made in the mpv window

`j` and `#` in mpv are the usual way to change tracks; `observe_track` (`mpv/command.rs`) observes `sid` and `aid`.

- **`MpvEvent::TrackChanged(kind)` carries no value.** Property changes are handled a file load after they are emitted, so the value is routinely stale (mpv's auto-selection lands after `configure_streams`). The runtime re-reads the live property and compares it with `settled`, which makes stale events no-ops.
- **One `TrackState { settled, remembered }` per kind**, picked by `track_state(kind)`; a `match` on kind only picks the field, and the logic runs once for either.
- **`settled` is mpv's selection the last time it was ours**, written by `apply_track` (optimistically) and `settle_track` (read back). Anything else reported is the user.
- **`SelectedTrack` (`mpv/event.rs`) is a tri-state.** Collapsing `Off` and `Unresolved` reads a loading file as the user switching subtitles off.

  | mpv answers      | `SelectedTrack` | Meaning                                   |
  | ---------------- | --------------- | ----------------------------------------- |
  | a number         | `Id(n)`         | This track is selected.                   |
  | `false` / `"no"` | `Off`           | Explicitly off. A decision.               |
  | `"auto"`, other  | `Unresolved`    | mpv has not picked yet. Never a decision. |

`adopt_mpv_track` (`runtime/playback/tracks.rs`):

1. Return while `Runtime::loading` is up or nothing is current; a loading file reports neither the old nor the new selection.
2. Live value equal to `settled` → nothing happened.
3. `Unresolved` → return; `Off` → `-1`; `Id(n)` → map back.
4. An id that maps to nothing (a sidecar the user loaded, external audio mpv found) becomes the new baseline so the event stops re-firing; nothing is reported or remembered.
5. Otherwise settle, remember by identity, update `current`'s stream index and send a progress report.

Subtitles check `external_subtitle_track_ids` before the embedded map; the order is arbitrary because `sub-add` ids sit above the embedded range.

## Lifetime

- **The memory lives on `Runtime`, which survives reconnects** (`specs/session.md`). Nothing reaches disk, and it holds only the latest pick per kind.

  | Cleared by           | `.remembered` | `.settled` | `external_subtitle_track_ids` |
  | -------------------- | ------------- | ---------- | ----------------------------- |
  | `start_current`      | no            | no         | yes                           |
  | `adopt_playlist_pos` | no            | no         | yes                           |
  | `configure_streams`  | no            | rewritten  | rebuilt                       |
  | `stop_playback`      | no            | yes        | yes                           |
  | WebSocket reconnect  | no            | no         | no                            |

- **`stop_playback` resets `settled` but not `remembered`.** `settled` means nothing once mpv is gone; stopping is not the user disowning their choice.

## mpv behaviour relied on

- `sid`/`aid` answer `false` or `"no"` for off and `"auto"` before mpv decides; only a number is a selection.
- `sub-add` appends a track above the embedded numbering **and selects it**.
- `observe_property` echoes its id; jellysink matches on the property name, so the two ids only have to differ.
- Embedded track ids are 1-based and in file order, per type; external audio gets no track.

## Known limits

- **One slot per kind, most recent only**, not per series and not persisted.
- **External audio is never selectable.** A request for one warns and mpv keeps its track.
- **The gate is a hard filter.** A provider that renames a track *and* changes its language falls back to the server default, deliberately.
- **`-1` from the remote is always Off**, including when the server sends it for `SubtitleMode=Default` with nothing flagged.
