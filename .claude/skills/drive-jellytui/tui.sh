#!/usr/bin/env bash
# Drives jellytui in a real terminal against the user's own Jellyfin. See SKILL.md.
set -euo pipefail

SOCK=jellytui-agent
TITLE=jellytui-agent
OUT=${TUI_OUT:-${TMPDIR:-/tmp}/jellytui-agent}
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
mkdir -p "$OUT"

t() { tmux -L "$SOCK" "$@"; }
win() { xdotool search --name "^$TITLE\$" | head -1; }
headless() { [[ $(t show -gv @headless 2>/dev/null) == 1 ]]; }
screen() { t capture-pane -p -t jt; }

# Stable for 0.4 s, and neither blank (startup probe) nor mid-load; gives up after 8 s.
settle() {
  local prev="" cur same=0
  for _ in $(seq 40); do
    sleep 0.2
    cur=$(screen)
    if [[ -z ${cur//[[:space:]]/} || $cur == *loading…* ]]; then
      same=0
    elif [[ $cur == "$prev" ]]; then
      ((++same >= 2)) && return 0
    else
      same=0
    fi
    prev=$cur
  done
}

# Kitty placeholders and halfblocks collapse to one ▒ per image, repeated rows go.
compact() {
  python3 -c '
import re, sys, unicodedata
last = None
for line in sys.stdin.read().splitlines():
    out, run = [], False
    for ch in line:
        if ch == "\U0010EEEE":
            if not run:
                out.append("▒")
            run = True
        elif run and unicodedata.category(ch) == "Mn":
            continue
        else:
            run = False
            out.append(ch)
    s = re.sub("[▀▄█]+(?: +[▀▄█]+)*", "▒", "".join(out)).replace("🮂", "")
    s = re.sub("─{3,}", "──", re.sub(" {3,}", "  ", s)).rstrip()
    if s != last:
        print(s)
    last = s
'
}

text() { if [[ ${1:-} == raw ]]; then screen; else screen | compact; fi; }

refuse() { echo "refused: $*" >&2; exit 2; }

case ${1:-help} in
start)
  t has-session -t jt 2>/dev/null && refuse "already running; '$0 stop' first"
  cols=170 rows=48 mode=gui
  for arg in "${@:2}"; do
    case $arg in
    --headless) mode=headless ;;
    *x*) cols=${arg%x*} rows=${arg#*x} ;;
    esac
  done
  bin=${JELLYTUI_BIN:-$(cd "$ROOT" && cargo build -q -p jellytui --message-format=json 2>/dev/null |
    sed -n 's/.*"executable":"\([^"]*jellytui\)".*/\1/p' | tail -1)}
  [[ -x $bin ]] || refuse "no jellytui binary (build failed?)"
  printf '%s\n' 'set -g allow-passthrough on' 'set -g status off' \
    'set -g default-terminal tmux-256color' 'set -g remain-on-exit on' \
    'set -g escape-time 10' >"$OUT/tmux.conf"
  if [[ $mode == headless ]]; then
    # Not looking like tmux makes tmux answer the graphics probe itself. A probe
    # nobody answers leaves a thread that eats the first key and drops raw mode.
    t -f "$OUT/tmux.conf" new-session -d -s jt -x "$cols" -y "$rows" \
      env -u TERM_PROGRAM TERM=xterm-256color "$bin"
    t set -g @headless 1
  else
    for dep in ghostty xdotool import; do
      command -v $dep >/dev/null || refuse "$dep missing; use --headless"
    done
    GDK_BACKEND=x11 setsid -f ghostty --title="$TITLE" --window-width="$cols" \
      --window-height="$rows" -e tmux -L "$SOCK" -f "$OUT/tmux.conf" \
      new-session -s jt "$bin" >/dev/null 2>&1
    for _ in $(seq 50); do [[ -n $(win 2>/dev/null) ]] && break; sleep 0.2; done
    [[ -n $(win) ]] || refuse "terminal window never appeared"
  fi
  settle
  text
  ;;
keys)
  shift
  for k in "$@"; do
    case $k in
    t | u | Enter | C-m | C-j | KPEnter) refuse "'$k' (use 'enter'; t and u are never safe)" ;;
    esac
  done
  for k in "$@"; do
    t send-keys -t jt "$k"
    for _ in $(seq 50); do
      sleep 0.15
      [[ $(screen) == *loading…* ]] || break
    done
  done
  settle
  text
  ;;
type)
  screen | grep -q 'type to search' || refuse "not on the search screen; press / first"
  t send-keys -t jt -l "$2"
  settle
  text
  ;;
enter)
  if [[ ${2:-} != --force ]] && screen | grep -qE '\b[0-9]+ min\b|Enter play'; then
    refuse "selection may be playable; Enter would start mpv and write play state to the user's server"
  fi
  t send-keys -t jt Enter
  settle
  text
  ;;
text) text "${2:-}" ;;
shot)
  headless && refuse "headless has no pixels; use 'text'"
  file="$OUT/${2:?name}.png"
  settle
  import -silent -window "$(win)" "$file"
  [[ -n ${3:-} ]] && magick "$file" -resize "$3%" "$file"
  echo "$file"
  ;;
size)
  cols=${2:?cols} rows=${3:?rows}
  if headless; then
    t resize-window -t jt -x "$cols" -y "$rows"
  else
    for _ in 1 2; do
      eval "$(xdotool getwindowgeometry --shell "$(win)")"
      read -r c r < <(t display -p '#{pane_width} #{pane_height}')
      xdotool windowsize "$(win)" $((WIDTH * cols / c)) $((HEIGHT * rows / r))
      sleep 0.5
    done
  fi
  settle
  t display -p '#{pane_width}x#{pane_height}'
  ;;
stop) t kill-server 2>/dev/null || true ;;
*) sed -n '/^## Commands/,/^## /p' "$(dirname "$0")/SKILL.md" ;;
esac
