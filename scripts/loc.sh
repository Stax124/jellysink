#!/usr/bin/env bash
# Rust line counts, production vs test, for the workspace and each crate.
set -euo pipefail

command -v tokei >/dev/null || { echo "loc.sh: tokei is not installed" >&2; exit 1; }
command -v jq >/dev/null || { echo "loc.sh: jq is not installed" >&2; exit 1; }

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# A test file is a sibling *_test.rs or anything under a tests/ directory.
sources() {
    local dir=$1 kind=$2
    case $kind in
        prod) find "$dir" -name target -prune -o -name '*.rs' \
                  ! -name '*_test.rs' ! -path '*/tests/*' -print ;;
        test) find "$dir" -name target -prune -o -name '*.rs' \
                  \( -name '*_test.rs' -o -path '*/tests/*' \) -print ;;
    esac
}

# code, comments, blanks, files for one set of sources; zeroes when the set is empty.
count() {
    local files
    mapfile -t files < <(sources "$1" "$2")
    ((${#files[@]})) || { echo "0 0 0 0"; return; }
    tokei -t Rust -o json "${files[@]}" \
        | jq -r '.Rust | "\(.code) \(.comments) \(.blanks) \(.reports | length)"'
}

row() {
    local name=$1 dir=$2
    read -r pc pm pb pf < <(count "$dir" prod)
    read -r tc tm tb tf < <(count "$dir" test)
    local total=$((pc + tc))
    local share=0
    ((total)) && share=$((tc * 100 / total))
    printf '%-14s %7d %7d %7d %7d  %5d%%\n' "$name" "$pc" "$tc" "$total" "$((pf + tf))" "$share"
    prod_comments=$pm prod_blanks=$pb test_comments=$tm test_blanks=$tb
}

printf '%-14s %7s %7s %7s %7s  %6s\n' '' 'prod' 'test' 'total' 'files' 'test%'
printf '%s\n' '───────────────────────────────────────────────────────────────'
for crate in "$root"/crates/*/; do
    row "$(basename "$crate")" "$crate"
done
printf '%s\n' '───────────────────────────────────────────────────────────────'
row 'workspace' "$root/crates"
printf '\nworkspace non-code lines: %d comments, %d blanks (prod) / %d, %d (test)\n' \
    "$prod_comments" "$prod_blanks" "$test_comments" "$test_blanks"

