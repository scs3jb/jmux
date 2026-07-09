# Producing the README screenshots (runbook for Claude / future sessions)

The screenshots in `docs/screenshots/*.png` are generated **headlessly and
privacy-safely** by [`docs/autocapture.sh`](autocapture.sh). This file explains
how it works, the hard-won gotchas, and how to regenerate or extend it.

## TL;DR

```bash
# needs: sway, grim, imagemagick (magick), poppler (pdftocairo), a release build
sudo pacman -S --needed sway grim
cargo build --release --features jmux/link-ghostty
bash docs/autocapture.sh        # writes docs/screenshots/*.png
```

Then **review every image** (e.g. read them back) before committing — privacy
is the whole point.

## The approach

jmux is GPU-accelerated GTK4 + ghostty (GLArea). On Wayland (esp. KDE) you can't
screenshot or control another app's window, and you don't want demo windows
flashing on the user's screen. So:

1. Run a **headless, GPU-backed compositor**: `WLR_BACKENDS=headless
   WLR_LIBINPUT_NO_DEVICES=1 sway` on a virtual 1600×1000 output. wlroots uses
   the real render node, so ghostty's GL renders correctly. Nothing appears on
   the user's screen.
2. Launch `target/release/jmux-app` as a Wayland client of that sway, in a
   **clean `env -i`** (no inherited secrets) with a **sandboxed `$HOME`**.
3. Drive every scene through the **jmux socket** (`jmux/bin/jmux` with
   `JMUX_SOCKET=$RT/jmux.sock`).
4. Capture each scene with **`grim`** (works with sway's wlr-screencopy).

Modals (overview, palette, task-manager) can't be dismissed via `jmux send`
(that goes to the terminal, not the dialog), so **relaunch jmux fresh per
scene** — sway stays up, jmux is killed and restarted for each capture. This
also guarantees no state bleeds between scenes.

## Privacy — the non-negotiable part

Everything must be synthetic. Real leaks that bit us and how they're avoided:

- **Login shell leaks `user@host` and `$HOME`.** ghostty spawns the user's real
  login shell via `getpwuid`, so a plain terminal shows `[jbriggs@radd-surfer …]`
  and a title `@radd-surfer:/home/jbriggs`. **Fix:** never show a plain shell.
  Every demo pane runs an **authored script** (`bash $SB/bin/<x>.sh`) that
  `clear`s, prints fake agent output, sets a clean title with
  `printf '\033]0;jmux\007'`, then `sleep`s. No prompt, no env, ever visible.
- **jmux launches TWO default workspaces** — a clean `Terminal` and a leaky
  `@radd-surfer:/home/jbriggs` (the shell's OSC title). After staging the demo
  workspaces, **close every workspace whose title isn't a demo name**
  (`web`/`api`/`docs`) via `jmux close <id>` (close-by-id is reliable;
  select-then-close is not).
- **Closing those leaky workspaces pollutes the History pane** (they land in the
  closed-stack). Before the history scene, run `jmux clear-closed`, then close a
  benign workspace so only that shows.
- **Vault** scans `~/.claude/projects` / `~/.codex/sessions`. The sandbox has
  **fake** sessions with `/home/demo` cwds and benign titles — never the real
  `~/.claude`.
- Paths shown are under `/tmp/jmux-demo` (no username). Acceptable.

## Look & feel (match the user's theme)

Read the user's theme prefs (theme prefs only — never their session/config with
real data) and replicate in the sandbox:

- `~/.config/jmux/settings.json` → `theme` (e.g. `dark`). Write a **minimal**
  `{"theme":"dark"}` to `$SB/.config/jmux/settings.json`. (A partial `sidebar:{…}`
  silently fails the parse and reverts to light — keep it minimal.)
- `~/.config/ghostty/config` → `theme = <name>` (e.g. `Adventure Time`). Write it
  to `$SB/.config/ghostty/config` **and copy the theme file** into
  `$SB/.config/ghostty/themes/` (the sandbox doesn't search `/usr/share/ghostty`).
- GTK font/dark → `$SB/.config/gtk-4.0/settings.ini`
  (`gtk-font-name`, `gtk-application-prefer-dark-theme`). libadwaita ignores the
  dark hint (use jmux `theme=dark`); the font does apply.

**Known gap:** libadwaita reads its **accent color** from the desktop settings
portal, which isn't running in headless sway — so the accent falls back to blue
instead of the user's. Matching it would require standing up a fake
`org.freedesktop.portal.Settings` advertising `accent-color`.

## The settings-vs-commands file trap

`~/.config/jmux/jmux.json` is **both** the settings file (strict,
`deny_unknown_fields`) **and** read for custom `commands`. You can't put both in
it. So:

- **Settings** → `$SB/.config/jmux/settings.json` (and do NOT create
  `$SB/.config/jmux/jmux.json`, or it wins and the `commands` key breaks the
  settings parse → defaults).
- **Commands** → `jmux.json` copied into **every** workspace dir's `.jmux/`
  (`$SB/.jmux/`, `$SB/web/.jmux/`, …). `custom_commands::load` searches the
  *selected* workspace's dir, which changes as you create workspaces, so the
  file must exist in all of them.

Multi-pane demo workspaces are built with a `commands[].workspace` layout
(recursive `split`/`pane`/`surfaces`) and launched via `jmux run <name>`.

## Scene list

`hero`, `pane-overview`, `command-palette`, `dock`, `vault-pane`,
`task-manager`, `pdf-preview`, `history-pane`. Triggers: `jmux overview` /
`palette` / `dock` / `top` / `vault` / `history`, `jmux open <pdf>`, and the
multi-pane `jmux run demo`. The dock needs a `dock.json`; the PDF a sample
generated with `magick … sample.pdf`.

## GIFs (also headless)

`docs/demos/{drag-tabs,move-panes,pane-overview,command-palette}.gif` are all
generated by [`docs/capture-tools/make-gifs.sh`](capture-tools/make-gifs.sh) —
drag, click, and keyboard interactions driven into a headless sway.

The trap: headless sway with `WLR_LIBINPUT_NO_DEVICES=1` has **no pointer device**
(seat `capabilities: 0`), so `swaymsg seat … cursor press/move` reports success
but reaches nothing — a synthetic click doesn't even change the workspace
selection. ydotool injects at the uinput/kernel level, which a `NO_DEVICES`
compositor ignores (and a nested input-grabbing sway on the user's live desktop
is unsafe).

What works: the **wlr-virtual-pointer protocol** (`zwlr_virtual_pointer_v1`). A
Wayland *client* creates a pointer the compositor treats as real, emitting proper
enter/motion/button events that **do** reach GTK and fire its drag-and-drop.
`wlrctl` does this but wasn't installed and system installs are disallowed, so
`capture-tools/` ships the protocol XML and a ~80-line C client (`vptr.c`); the
script builds it with `wayland-scanner` + `cc`. `vptr` reads a drag script on
stdin (`m X Y` / `d` / `u` / `w MS`) and **keeps one pointer alive for the whole
press→move→release** — essential, since GTK DnD needs a continuous button hold
across motion.

For the **command palette** the same trick extends to typing: a
`zwp_virtual_keyboard_v1` client (`vkbd.c`) uploads a US xkb keymap (built with
libxkbcommon) and sends key events that reach the palette's GTK search entry.
The palette is opened via the socket (`jmux palette`) and `vkbd` types the query
so the list fuzzy-filters live, then Enter switches. The **first keystroke after
the entry grabs focus is dropped**, so the script warms up with a no-op Backspace
(`k 14`) before typing the real text.

Gotchas baked into the script:
- Verify the pointer reaches GTK first (a click must change the sidebar
  selection) before trusting any drag.
- Each tab needs a **distinct title** (its own OSC-titled script) or a reorder is
  invisible — background ghostty tabs render their script lazily, so they show
  "Terminal" until focused; dragging the one distinctly-titled tab still reads.
- There's a **release→reflow lag**: jmux finishes re-laying-out ~0.5s after the
  drop, so keep capturing frames well past the `u` (the move-panes script holds
  1.6s) or the GIF ends before the panes merge.
- GIF frames are deltas — extract single frames with `magick gif -coalesce`, not
  `gif[N]`, or you'll see only the changed strip (looks blank/garbled). (Watch the
  glob too: `ls foo_*.png | tail -1` picks `foo_last.png` over `foo_142.png`
  because `l` > `1`.)
- **No Vault GIF on purpose.** Clicking a Vault session runs the *real* `claude`
  binary (`--resume`), which is non-deterministic and can surface real state on
  another machine — never drive it. The search-filter is safe, but the headless
  GLArea leaves faint ghost rows of the filtered-out sessions (a source render
  artifact, not a palette issue — full-stats palette doesn't fix it), so the
  static `vault-pane.png` screenshot is used instead.
- Crops always start at **y=46–48** so the **title bar is excluded** (it can carry
  the leaky `@host:/home/...` window title). The drag GIFs also drop the sidebar
  (x=285); the overview/palette GIFs keep it (x=0) for context — safe because the
  staged workspace titles are synthetic (`web`/`api`/`docs`/`fleet`). Only fake
  pane content and synthetic `/tmp/jmux-demo` paths ever appear.

## Always do last

Read back **every** generated PNG and confirm: no real username/hostname, no
`/home/<user>`, no real session content, the leaky default workspace is gone,
and the History pane shows only benign closed entries.
