---
name: drive-jellytui
description: Run jellytui against the user's real Jellyfin and use it like a user would (keypresses, screen text, real cover-image screenshots). Use it to check TUI output, layout or navigation by eye, before or after a jellytui change.
---

# Driving jellytui

`tui.sh` (next to this file) builds jellytui and runs it under tmux inside Ghostty, so kitty graphics render the real covers. Every command waits for the screen to settle, then prints it as compact text. It uses the user's own config and server; don't pass config overrides.

## Safety (it is the user's real server and screen)

- **Enter on anything that is not a container plays it.** mpv opens on the user's desktop, and PlayCount and LastPlayedDate change on their server. `keys` refuses Enter. `enter` refuses when the screen shows a runtime (`N min`) or an `Enter play` hint. Use `enter --force` only with the user's consent.
- Never press `t` (writes watched state) or `u` (self-update: replaces installed binaries and restarts the daemon). `keys` refuses both.
- If something starts playing anyway, quit mpv: `echo '{"command":["quit"]}' | socat - ~/.config/jellysink/mpv.sock` (or Python `socket`). Then tell the user.
- To reach a series from Search, go through `2` Libraries. Enter on a search result is refused, because the screen always shows `Enter play` there.

## Commands

```
S=.claude/skills/drive-jellytui/tui.sh
$S start [COLSxROWS] [--headless]   # default 170x48; --headless = tmux only, no window, no shots
$S keys Right Down 2 Escape /        # tmux key names; prints the screen
$S type frieren                      # literal text, search screen only
$S enter [--force]                   # guarded Enter
$S text [raw]                        # compact (default) or exact screen
$S shot NAME [PCT]                   # PNG of the window → path; Read it to see covers
$S size COLS ROWS                    # resize (try 80 24 for small-terminal layout)
$S stop
```

Keys: `1` Home, `2` Libraries (resets the browse stack), `3` Playing, `/` Search, `L` Logs, `Escape` back, `r` reload, arrow keys, `PageUp`/`PageDown`, `Home`/`End`.

## Token tips

- Read the screen from `text`. Take `shot` only when pixels matter (covers, colour, alignment), and prefer `shot NAME 60`: about a third of the tokens, still legible.
- In compact text, `▒` is one image and repeated rows are dropped, so card positions show once. Selection and progress bars are colour only, visible just in a `shot`. The detail pane title tells you which item is selected.
- Batch several keys per `keys` call. Each call prints only the final screen.
- `--headless` is enough for navigation or layout checks with no images. Covers then fall back to halfblocks.

## Gotchas

- Needs X/XWayland (`DISPLAY`), `ghostty`, `tmux`, `xdotool`, ImageMagick. Otherwise use `--headless`.
- `shot` captures only that window via `import -window`. Never use `spectacle`, `grim` or a full-screen grab: they capture whatever the user has on screen.
- Ghostty runs under XWayland (`GDK_BACKEND=x11`) so that `xdotool` can find and resize it. The tmux conf sets `allow-passthrough on`, which kitty graphics need.
- A blank grid next to a filled synopsis is the app at that size, not the harness. Compare at the default size.
- Always `stop` at the end. The tmux socket is `-L jellytui-agent`, and files go to `$TUI_OUT` (default `/tmp/jellytui-agent`).
