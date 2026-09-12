# Audio and subtitle tracks

How jellysink maps Jellyfin stream indexes to mpv tracks, and how a track the
user picked survives into the next episode.

Two problems shape all of it. Jellyfin stream indexes and mpv track ids are
different numbering systems that only sometimes agree; and the server's
`DefaultAudioStreamIndex` / `DefaultSubtitleStreamIndex` is the thing being
worked around in the first place — for a mislabeled release it points at the
wrong track, and for a release that splits one language into `Signs and Songs`
and `Dialogue` it regularly picks the wrong half.

## The numbering systems

| Number                   | Whose  | Scope                                                                |
| ------------------------ | ------ | -------------------------------------------------------------------- |
| Jellyfin stream index    | server | One sequence over *all* streams of an item — video, audio, subtitle. |
| mpv `aid`                | mpv    | 1-based, per type, **muxed audio only**, in file order.              |
| mpv `sid` (embedded)     | mpv    | 1-based, per type, in file order.                                    |
| mpv `sid` (`sub-add`ed)  | mpv    | Assigned on load, above the embedded range.                          |

Everything the rest of the code speaks — `PlayRequest`, the `PlayingState`
reports, `CastEvent::SetAudio` / `SetSubtitle` — is a Jellyfin stream index.
mpv only ever sees a track id. `StreamMaps` (`media/streams.rs`) is the whole
translation layer:

| Field                               | Direction                                   |
| ----------------------------------- | ------------------------------------------- |
| `audio_track_id_by_stream_index`    | Jellyfin index → `aid`                      |
| `subtitle_track_id_by_stream_index` | Jellyfin index → embedded `sid`             |
| `subtitle_url`                      | Jellyfin index → absolute URL to `sub-add`  |
| `audios` / `subtitles`              | every stream mpv can actually be pointed at |

The reverse directions (`jellyfin_embedded_audio_index`,
`jellyfin_embedded_subtitle_index`) are linear scans of those maps — one entry
per stream, read once per track change, so a second `HashMap` kept in sync is
not worth it.

`sub-add`ed subtitles are the exception: their ids are not in `StreamMaps` but
in `Runtime::external_subtitle_track_ids`, because they only exist once mpv has
loaded the file. That map is rebuilt per file.

## Building the maps (`map_streams`)

One pass per kind over `MediaSource.MediaStreams`.

**Audio: external streams are skipped entirely.** mpv has a track only for
audio muxed into the file it is playing. Mapping an external stream anyway
hands back the `aid` of the *next* embedded track and plays the wrong audio. It
is kept out of `audios` too, so it can never become a remembered choice.

**Subtitles have two independent axes**, and this is the subtlest part of the
file. `DeliveryMethod` and `IsExternal` look like the same question:

| Axis             | Decides                                                    |
| ---------------- | ---------------------------------------------------------- |
| `DeliveryMethod` | How *Jellyfin* hands it over — an in-file track, or a URL. |
| `IsExternal`     | Whether *mpv* has an in-file track for it.                 |

Jellyfin reports an in-file subtitle as `External` when it has to extract it to
a sidecar, and mpv still has an in-file track for it. So the branch deciding
where the entry goes is on `DeliveryMethod`, and the branch advancing the mpv
`sid` counter is on `IsExternal`. Gating the counter on the delivery method
skips a number mpv did allocate, and every later subtitle in the file resolves
one track off.

`selectable` gates entry into `subtitles`. Its two `warn` arms — an `External`
stream with no `DeliveryUrl`, and an unrecognised delivery method — are streams
we cannot point mpv at, so they must never become a remembered choice either:
the user would pick one and every later episode would silently fall back to the
server default.

## Remembering a choice

### Why not the index

Stream indexes are per-file. The next episode can order its streams
differently, or come from another provider, so "index 3" is not the same track
twice. A choice is stored as an *identity* — `TrackId` (`media/track.rs`) — and
re-matched against whatever the next item offers.

`TrackId` is one struct used for both kinds; `SubtitleId` and `AudioId` are
aliases of it, and only which `Runtime` field a value lands in keeps the two
memories apart. `is_forced` and `is_external` are subtitle notions, always
`false` for audio, so their weights add the same constant to every audio
candidate and cannot change a ranking.

### What counts as a name

`named()` returns `None` for `""`, `"und"` and `"undefined"` as well as for an
absent field: Jellyfin sends those rather than omitting them, and treating them
as values would make every unlabelled track match every other one. `same()`
requires both sides present — two absences are never a match.

### Scoring

`score` first *gates*: a candidate must agree on language, title or display
title, or it is not recognisably the same track and scores `None`. Flags, codec
and index only rank candidates that already passed — on their own they would
match an unrelated stream, since every non-forced embedded SRT agrees with
every other one.

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

The magnitudes are deliberately non-overlapping so the sum behaves
lexicographically: everything below `LANGUAGE` adds up to 875, so no pile of
name and flag agreements can outrank the language. Playing the wrong language
is a far worse failure than playing the wrong track within the right one.

`best_match` takes the highest score, and a tie goes to the **lowest index**, so
an item with two indistinguishable tracks resolves the same way every time.

### What cannot be remembered

`TrackPreference::from_selection` returns `None` — *forgetting* the previous
choice — for an index this item cannot select (not in `candidates`), and for a
stream carrying neither a language nor a name (`is_identifiable`). Forgetting
rather than keeping is deliberate: an identity that can never match again would
leave the earlier choice alive, and silently re-applying a track the user has
already moved away from is the most confusing outcome available.

`stream_index < 0` never consults the candidate list — Off is a decision even
for an item with no streams of that kind at all.

## Applying it

### Precedence

`resolve_track_index` (`media/track.rs`), highest first:

| Rank | Source                    | Why                                                                            |
| ---- | ------------------------- | ------------------------------------------------------------------------------ |
| 1    | `requested`               | The remote named a stream for *this* item; nothing we remember outranks that.  |
| 2    | The remembered preference | Only when this item has a matching track.                                      |
| 3    | `server_default`          | The fallback.                                                                  |

`audio.rs` and `subtitle.rs` are two thin wrappers that fix `TrackKind` and give
the caller a kind-named function; the matching itself is one copy of the code.

`TrackPreference::Off` resolves to `-1`, an explicit *no* rather than
"unspecified". For subtitles it has to: `sub-add` selects the track it adds, so
leaving `sid` unset shows the last external subtitle instead of none. For audio
it is reachable through mpv's `cycle audio` (`#`), which cycles past the last
track into "no audio".

### The one place it happens

`prepare_item` (`runtime/queue/mod.rs`) is the only producer of a
`PreparedPlay`, so it is also the only place the memory is applied
(`with_remembered_tracks`). Both `start_current` and `adopt_playlist_pos` come
through it — which is what lets a remembered track reach a playlist jump and
mpv's own autoplay, the path a series actually takes between episodes.

**The cache stores the server's answer, not the overridden one.** `prepare_item`
inserts the raw `PreparedPlay` into `prepared` and applies the memory to the
returned clone. Caching the overridden one would make a later fallback mean
"whatever the preference was the first time this episode played" instead of
"what the server said".

### Ordering inside `configure_streams`

Runs on `file-loaded`, and the order matters:

1. `sub-add` every external subtitle URL, recording the new `sid` from
   `max_subtitle_track_id` after each one. Each `sub-add` *selects* the track it
   just added, so nothing before step 3 can be trusted as a selection.
2. Apply the resolved audio index.
3. Apply the resolved subtitle index.
4. **Read** `sid` and `aid` back and record them as settled.

Step 4 reads rather than assumes: with no index to apply, the selection is
mpv's own — its config's default track, or the last `sub-add`ed one.

## Noticing a pick made in the mpv window

A Jellyfin client is not the only way to change tracks. `j` and `#` in the mpv
window are, and in practice they are the usual way. mpv reports both as a
property change on `sid` / `aid`, registered by `observe_subtitle_track` and
`observe_audio_track` (`mpv/command.rs`).

### Why the event carries no value

`MpvEvent::SubtitleTrackChanged` and `MpvEvent::AudioTrackChanged` deliberately
carry no track id. Property changes arrive on their own channel and are handled
a whole file load later than they were emitted, so the value in the message is
routinely stale: mpv's auto-selection during a load reaches the runtime *after*
`configure_streams` has already applied our choice over it. The runtime re-reads
the live property and compares it against the selection it last settled on,
which turns every stale event into a no-op.

### One `TrackState` per kind

```rust
struct TrackState {
    settled: SelectedTrack,
    remembered: Option<TrackPreference>,
}
```

`Runtime` holds one for `audio` and one for `subtitle`; `track_state(kind)` /
`track_state_mut(kind)` (`runtime/playback/tracks.rs`) are the only places that
match on `kind`. Everything else — `apply_track`, `adopt_mpv_track`,
`remember_track`, `settle_track` — takes a `TrackKind` and runs the same code
for either.

`settled` is mpv's selection as of the last time it was *ours*: written
optimistically by `apply_track` (to what it asked mpv for) and by `settle_track`
(by reading mpv back). A property change reporting anything else is the user
reaching for the track menu.

`SelectedTrack` (`mpv/event.rs`) is a tri-state, because mpv's answer is not
just a number:

| mpv answers      | `SelectedTrack` | Meaning                                   |
| ---------------- | --------------- | ----------------------------------------- |
| a number         | `Id(n)`         | This track is selected.                   |
| `false` / `"no"` | `Off`           | Explicitly off. A decision.               |
| `"auto"`, other  | `Unresolved`    | mpv has not picked yet. Never a decision. |

Collapsing `Off` and `Unresolved` into "no track" reads a file that is still
loading as the user switching subtitles off.

### `adopt_mpv_track` (`runtime/playback/tracks.rs`)

1. Return early while `transitioning` or `stopping`, or with no current item — a
   file still loading reports the selection of neither the old file nor the
   configured new one.
2. Read the live property. If it equals the settled value, nothing happened.
3. `Unresolved` → return; `Off` → Jellyfin index `-1`; `Id(n)` → map back.
4. A track id that maps back to nothing — a sidecar the user loaded themselves,
   an external audio file mpv picked up beside the stream — becomes the new
   baseline and returns. There is nothing to report and nothing that could be
   re-found in the next episode, but it *is* what is on screen, so recording it
   stops the event re-firing.
5. Otherwise: settle, remember by identity, update `current`'s stream index so
   the Jellyfin UI stops showing a track that is not playing, and send a
   progress report.

Subtitles consult `external_subtitle_track_ids` before the embedded map; audio
has one lookup, since mpv never loads an external audio stream. The subtitle
lookup order is arbitrary — `sub-add` appends, so an external track's id sits
above the embedded numbering and the two cannot collide.

## Lifetime of the memory

`Runtime` is built once in `runtime::run` and reused across every WebSocket
reconnect for the life of the daemon (see `specs/session.md`), so
`TrackState.remembered` survives a reconnect simply by living on it. Nothing
here reaches disk: the preference is in memory for as long as the daemon runs
and holds only the most recent selection per kind.

| Cleared by           | `.remembered` | `.settled` | `external_subtitle_track_ids` |
| -------------------- | ------------- | ---------- | ----------------------------- |
| `start_current`      | no            | no         | yes                           |
| `adopt_playlist_pos` | no            | no         | yes                           |
| `configure_streams`  | no            | rewritten  | rebuilt                       |
| `stop_playback`      | no            | yes        | yes                           |
| WebSocket reconnect  | no            | no         | no                            |

`stop_playback` resets `.settled` to `Unresolved` because it is a
per-mpv-session value with no meaning once mpv is gone. `.remembered` is a
`Runtime` concept rather than a playback one and outlives it on purpose —
stopping playback is not the user disowning their choice.

## mpv behaviour this relies on

- `sid` / `aid` answer `false` or `"no"` for an explicit off and `"auto"` before
  mpv has decided; only a number is a selection.
- `sub-add` appends a track above the embedded numbering **and selects it**.
- `observe_property` echoes back the id it was given; jellysink matches on the
  property name instead, so the only thing that matters is that the two ids
  differ.
- Embedded track ids are 1-based and in file order, per type.
- mpv has no track for an external audio stream.

## Known limits

- **One slot per kind, most recent only.** Not per series, not per language
  pair, and not persisted — restarting the daemon forgets it.
- **External audio is never selectable.** A Jellyfin client asking for one gets
  the "not found in the embedded audio map" warning and mpv keeps what it had.
- **A track only mpv knows about cannot be remembered.** It becomes the baseline
  so events stop re-firing, but there is no identity to match next episode.
- **The gate is a hard filter.** A provider that renames a track *and* changes
  its language tag matches nothing and falls back to the server default. That is
  intended: a wrong-language match is worse than no match.
- **`-1` from the remote is Off, not "unspecified".** The server also sends `-1`
  when `SubtitleMode=Default` and nothing is flagged default/forced/external;
  that is still a decision of Off.

## Where this is tested

Matching and forgetting in `media/track_test.rs`; the two `sid`/`aid` counter
gates above in `media/streams_test.rs`; the precedence table once per side in
`media/audio_test.rs` and `media/subtitle_test.rs`; the tri-state and the
observers in `mpv/event_test.rs` and `mpv/command_test.rs`. Anything that turns
out to be mpv behaving other than as described above belongs in
`mpv/integration_test.rs`.
