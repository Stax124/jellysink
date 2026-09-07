#!/usr/bin/env bash
# Regenerates the media fixtures the real-mpv integration tests play.
# Requires ffmpeg. Run from the repository root:
#
#   ./tests/fixtures/make-fixtures.sh
#
# sample.mkv is 3 seconds of test pattern with two audio tracks (mpv aid 1, 2)
# and two embedded subtitle tracks (mpv sid 1, 2), so track selection has
# something to select. Kept tiny (crf 40, 160x120, 10 fps) because it is
# committed to the repository.
set -euo pipefail

cd "$(dirname "$0")"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf '1\n00:00:00,000 --> 00:00:03,000\nembedded english\n\n' >"$tmp/eng.srt"
printf '1\n00:00:00,000 --> 00:00:03,000\nembedded czech\n\n' >"$tmp/ces.srt"

ffmpeg -v error -y \
    -f lavfi -i "testsrc=duration=3:size=160x120:rate=10" \
    -f lavfi -i "sine=frequency=440:duration=3" \
    -f lavfi -i "sine=frequency=660:duration=3" \
    -i "$tmp/eng.srt" -i "$tmp/ces.srt" \
    -map 0:v -map 1:a -map 2:a -map 3:s -map 4:s \
    -c:v libx264 -preset veryfast -crf 40 -pix_fmt yuv420p \
    -c:a aac -b:a 32k -c:s srt \
    -metadata:s:a:0 language=eng -metadata:s:a:0 title="Stereo" \
    -metadata:s:a:1 language=ces -metadata:s:a:1 title="Commentary" \
    -metadata:s:s:0 language=eng -metadata:s:s:0 title="English" \
    -metadata:s:s:1 language=ces -metadata:s:s:1 title="Czech" \
    sample.mkv

# The external subtitle `sub_add` loads; a third sid on top of the two above.
printf '1\n00:00:00,000 --> 00:00:03,000\nexternal subtitle\n\n' >external.srt
