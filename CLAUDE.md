# jmux

## Building

ALWAYS build the release binary with **all link features** enabled, so the
installed build has ghostty linked AND the quake-style quick-terminal:

```sh
cargo build --release --features jmux/link-ghostty,jmux/quick-terminal
```

- `link-ghostty` — links the ghostty terminal library (required).
- `quick-terminal` — quake drop-down + global hotkey/portal. `install.sh`
  installs whatever was last built, so omitting this silently ships a build
  without the hotkey. Keep it on.
- `webkit` is on by default (browser panels).

Do not use a plain `cargo build --release` for anything that will be installed.

## Installing

`scripts/install.sh` copies `target/release/jmux-app` + the `jmux`/`jmux-cli`
wrappers and shell integration into a prefix (default `/usr/local`, needs root):

```sh
sudo bash scripts/install.sh
```

Build first (above) — the script refuses to run if `target/release/jmux-app`
is missing.
