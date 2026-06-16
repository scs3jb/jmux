#!/usr/bin/env bash
# Install cmux-gtk into a prefix using the layout the app expects:
#
#   $PREFIX/bin/cmux-app                      GUI binary
#   $PREFIX/bin/cmux                          CLI (bash wrapper)
#   $PREFIX/bin/cmux-cli                      typed CLI client (Rust)
#   $PREFIX/share/cmux/shell-integration/     zsh/bash/fish integration (+ .zshenv)
#   $PREFIX/share/cmux/bin/                    cmux/claude/xdg-open (PATH-injected in cmux terminals)
#   $PREFIX/share/applications/…desktop       launcher entry
#   $PREFIX/share/metainfo/…metainfo.xml      AppStream metadata
#   $PREFIX/share/icons/hicolor/scalable/…    bundled symbolic icons
#
# The GUI resolves shell-integration via <exe_dir>/../share/cmux/shell-integration,
# so this layout works without any extra configuration.
#
# Usage:
#   bash scripts/install.sh [PREFIX]      # default PREFIX=/usr/local (needs root)
#   bash scripts/install.sh "$HOME/.local"  # per-user, no root required
set -euo pipefail

PREFIX="${1:-/usr/local}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REL="$REPO_ROOT/target/release"

if [[ ! -x "$REL/cmux-app" ]]; then
  echo "error: $REL/cmux-app not found — build first with:" >&2
  echo "  cargo build --release --features cmux/link-ghostty" >&2
  exit 1
fi

echo "Installing cmux-gtk to $PREFIX"

install -Dm755 "$REL/cmux-app" "$PREFIX/bin/cmux-app"
# Bash CLI wrapper is the canonical `cmux` (matches the in-terminal PATH prepend).
install -Dm755 "$REPO_ROOT/cmux/bin/cmux" "$PREFIX/bin/cmux"
# Typed Rust CLI client (optional companion).
[[ -x "$REL/cmux" ]] && install -Dm755 "$REL/cmux" "$PREFIX/bin/cmux-cli"

# Shell integration (preserve dotfiles like .zshenv and the fish/ subtree).
dest_si="$PREFIX/share/cmux/shell-integration"
rm -rf "$dest_si"
mkdir -p "$dest_si"
cp -a "$REPO_ROOT/cmux/shell-integration/." "$dest_si/"

# Bundled bin scripts (cmux/claude/xdg-open) for the in-terminal PATH prepend.
dest_bin="$PREFIX/share/cmux/bin"
mkdir -p "$dest_bin"
cp -a "$REPO_ROOT/cmux/bin/." "$dest_bin/"
chmod 755 "$dest_bin"/*

# Desktop entry + AppStream metadata.
install -Dm644 "$REPO_ROOT/data/io.github.douglas.cmux_gtk.desktop" \
  "$PREFIX/share/applications/io.github.douglas.cmux_gtk.desktop"
install -Dm644 "$REPO_ROOT/data/io.github.douglas.cmux_gtk.metainfo.xml" \
  "$PREFIX/share/metainfo/io.github.douglas.cmux_gtk.metainfo.xml"

# Bundled symbolic icons into the hicolor theme so GTK resolves them.
if [[ -d "$REPO_ROOT/cmux/icons/scalable" ]]; then
  while IFS= read -r -d '' svg; do
    rel="${svg#"$REPO_ROOT"/cmux/icons/scalable/}"
    install -Dm644 "$svg" "$PREFIX/share/icons/hicolor/scalable/$rel"
  done < <(find "$REPO_ROOT/cmux/icons/scalable" -name '*.svg' -print0)
fi

# Application icon(s) into the hicolor theme (matches the desktop Icon=).
# Install both the scalable SVG and the sized PNG renditions — some docks /
# taskbars only resolve a pinned (not-running) launcher's icon via the PNG
# sizes in the icon cache, showing a generic icon otherwise.
if [[ -d "$REPO_ROOT/data/icons/hicolor" ]]; then
  while IFS= read -r -d '' icon; do
    rel="${icon#"$REPO_ROOT"/data/icons/hicolor/}"
    install -Dm644 "$icon" "$PREFIX/share/icons/hicolor/$rel"
  done < <(find "$REPO_ROOT/data/icons/hicolor" \
    \( -name '*.svg' -o -name '*.png' -o -name 'index.theme' \) -print0)
  # Pixmaps fallback — some launchers look here directly by Icon= name.
  install -Dm644 "$REPO_ROOT/data/icons/hicolor/256x256/apps/io.github.douglas.cmux_gtk.png" \
    "$PREFIX/share/pixmaps/io.github.douglas.cmux_gtk.png" 2>/dev/null || true
fi

# Refresh caches (best-effort).
command -v update-desktop-database >/dev/null 2>&1 && \
  update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
  gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true

# NOTE: do NOT rebuild the user's KDE sycoca/icon cache from this (root)
# installer — running kbuildsycoca without the user's session environment can
# corrupt their caches. KDE/Plasma will pick up the new icon on the next login,
# or the user can run `kbuildsycoca6` themselves from within their session.

echo "Done."
echo "  GUI : $PREFIX/bin/cmux-app"
echo "  CLI : $PREFIX/bin/cmux"
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "  note: $PREFIX/bin is not on your PATH — add it to run 'cmux-app' by name." ;;
esac
