# Capturing README screenshots & demo GIFs

The images referenced by the README live in `docs/screenshots/*.png` and
`docs/demos/*.gif`. They currently ship as **labeled placeholders** — replace
them with real captures of the running app.

A helper script is provided: [`docs/capture.sh`](capture.sh).

```bash
# Screenshot (select a region with the mouse):
docs/capture.sh shot <name>

# Demo GIF (records a region for N seconds, default 8):
docs/capture.sh gif <name> [seconds]
```

It uses **Spectacle** (KDE) or `grim`+`slurp` for screenshots, and
`wf-recorder`+`slurp` (Wayland) + `ffmpeg`/`gifski` for GIFs. Install whatever
your session has:

```bash
# Arch / KDE
sudo pacman -S spectacle grim slurp wf-recorder ffmpeg gifski
```

## What to capture

### Screenshots (`docs/screenshots/<name>.png`)

| name | show… |
|------|-------|
| `hero` | the main window with a few split panes / agents running |
| `pane-overview` | the Pane Overview grid (header grid button) with several panes |
| `agent-integrations` | `jmux claude-teams` or `jmux omo` with teammates as panes |
| `history-pane` | `jmux history` with some closed/focused entries |
| `vault-pane` | `jmux vault` listing past sessions |
| `dock` | the Dock open with a couple of controls (e.g. lazygit + a log tail) |
| `dock-editor` | Settings → Dock → Edit Dock Controls |
| `textbox` | a terminal with the TextBox composer below it |
| `pdf-preview` | the file-preview panel showing a PDF (or an image) |
| `task-manager` | `jmux top` |
| `command-palette` | the palette open (`Ctrl+Shift+P`) with custom commands listed |

### Demo GIFs (`docs/demos/<name>.gif`)

| name | record… |
|------|---------|
| `drag-tabs` | dragging a tab to reorder it, then onto a pane edge to split |
| `move-panes` | dragging a pane between splits / to another workspace |

## Tips

- Keep GIFs short (5–10 s) and the region tight; `capture.sh gif` scales to
  ~1000px wide to keep file sizes reasonable.
- Use a clean theme and a couple of real agents for the hero shot.
- Commit the resulting PNG/GIF files (they replace the placeholders in place).
