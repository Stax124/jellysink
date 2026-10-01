#!/usr/bin/env bash
# Starts the disposable Jellyfin the live-server tests run against: one series,
# Specials plus four seasons, 106 episodes, every file a hardlink to the 3 s
# fixture and every title from an NFO, so the scan never goes online.
#
#   testserver/start.sh     # then `cargo nextest run`
#   testserver/stop.sh
#
# CONTAINER=podman swaps the runtime; JELLYSINK_TEST_PORT (default 8096) moves
# the port, and the tests read the same variable.
set -euo pipefail

CONTAINER="${CONTAINER:-docker}"
PORT="${JELLYSINK_TEST_PORT:-8096}"
# 12.1.0, pinned to the build rather than the moving `12.1` tag.
IMAGE="docker.io/jellyfin/jellyfin:12.1.20260915-010956"
NAME="jellysink-test-jellyfin"
USERNAME="jellysink"
PASSWORD="jellysink"
SERIES="That Time I Got Reincarnated as a Slime"
EPISODES=106

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(dirname "$here")"
media="$repo/target/testserver/media"
series_dir="$media/Shows/$SERIES"
sample="$repo/crates/jellysink/tests/fixtures/sample.mkv"
server="http://127.0.0.1:$PORT"
client='MediaBrowser Client="jellysink-testserver", Device="testserver", DeviceId="jellysink-testserver", Version="1"'

xml_escape() {
    local s="$1"
    s="${s//&/&amp;}"
    s="${s//</&lt;}"
    s="${s//>/&gt;}"
    printf '%s' "$s"
}

# `wait_for <what> <seconds> <command...>`: retries once a second, then fails.
wait_for() {
    local what="$1" seconds="$2"
    shift 2
    for _ in $(seq "$seconds"); do
        if "$@"; then
            return 0
        fi
        sleep 1
    done
    echo "timed out after ${seconds}s waiting for $what" >&2
    "$CONTAINER" logs --tail 100 "$NAME" >&2 || true
    exit 1
}

rm -rf "$media"
mkdir -p "$series_dir"
cp "$here/tvshow.nfo" "$here/poster.jpg" "$series_dir/"
while IFS=$'\t' read -r season episode title; do
    season_dir="$series_dir/$(printf 'Season %02d' "$season")"
    if [[ ! -d "$season_dir" ]]; then
        mkdir -p "$season_dir"
        season_title="Season $season"
        [[ "$season" == 0 ]] && season_title="Specials"
        printf '<?xml version="1.0" encoding="utf-8"?>\n<season>\n  <title>%s</title>\n  <seasonnumber>%s</seasonnumber>\n</season>\n' \
            "$season_title" "$season" >"$season_dir/season.nfo"
    fi
    base="$season_dir/$(printf '%s - S%02dE%02d' "$SERIES" "$season" "$episode")"
    ln "$sample" "$base.mkv" 2>/dev/null || cp "$sample" "$base.mkv"
    printf '<?xml version="1.0" encoding="utf-8"?>\n<episodedetails>\n  <title>%s</title>\n  <season>%s</season>\n  <episode>%s</episode>\n</episodedetails>\n' \
        "$(xml_escape "$title")" "$season" "$episode" >"$base.nfo"
done <"$here/episodes.tsv"

"$CONTAINER" rm -f "$NAME" >/dev/null 2>&1 || true
"$CONTAINER" run -d --name "$NAME" -p "$PORT:8096" -v "$media:/media:ro" "$IMAGE" >/dev/null

wait_for "Jellyfin to answer on $server" 120 \
    curl -fs -o /dev/null "$server/System/Info/Public"

post() {
    local path="$1" body="$2"
    shift 2
    curl -fsS -X POST -H "Content-Type: application/json" -H "Authorization: $client" "$@" \
        --data "$body" "$server$path"
}

# The startup API only answers once the server has finished its own startup.
wait_for "the startup wizard" 60 \
    curl -fs -o /dev/null -H "Authorization: $client" "$server/Startup/User"
post /Startup/Configuration '{"UICulture":"en-US","MetadataCountryCode":"US","PreferredMetadataLanguage":"en"}'
post /Startup/User "{\"Name\":\"$USERNAME\",\"Password\":\"$PASSWORD\"}"
post /Startup/RemoteAccess '{"EnableRemoteAccess":true}'
post /Startup/Complete '{}'

token="$(post /Users/AuthenticateByName "{\"Username\":\"$USERNAME\",\"Pw\":\"$PASSWORD\"}" |
    sed -n 's/.*"AccessToken":"\([^"]*\)".*/\1/p')"
[[ -n "$token" ]] || {
    echo "logging in as $USERNAME returned no token" >&2
    exit 1
}
auth="$client, Token=\"$token\""

offline='"MetadataFetchers":[],"ImageFetchers":[]'
curl -fsS -X POST -H "Content-Type: application/json" -H "Authorization: $auth" \
    --data "{\"LibraryOptions\":{
        \"EnableRealtimeMonitor\":false,
        \"SaveLocalMetadata\":false,
        \"MetadataSavers\":[],
        \"EnableChapterImageExtraction\":false,
        \"ExtractChapterImagesDuringLibraryScan\":false,
        \"EnableTrickplayImageExtraction\":false,
        \"PathInfos\":[{\"Path\":\"/media/Shows\"}],
        \"TypeOptions\":[
            {\"Type\":\"Series\",$offline},
            {\"Type\":\"Season\",$offline},
            {\"Type\":\"Episode\",$offline}]}}" \
    "$server/Library/VirtualFolders?name=Shows&collectionType=tvshows"
# `refreshLibrary=true` above scans before the new folder is registered, and finds nothing.
curl -fsS -X POST -H "Authorization: $auth" "$server/Library/Refresh"

episodes_scanned() {
    curl -fs -H "Authorization: $auth" \
        "$server/Items?Recursive=true&IncludeItemTypes=Episode&Limit=0" |
        grep -q "\"TotalRecordCount\":$EPISODES[,}]"
}
wait_for "the library scan to find $EPISODES episodes" 180 episodes_scanned

echo "Jellyfin $IMAGE is up on $server as $USERNAME/$PASSWORD with $EPISODES episodes"
