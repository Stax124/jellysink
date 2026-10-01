#!/usr/bin/env bash
# Removes the container `testserver/start.sh` started.
set -euo pipefail

"${CONTAINER:-docker}" rm -f jellysink-test-jellyfin >/dev/null
