#!/usr/bin/env bash
# Build the release binaries with every link feature on, so the installed
# build has ghostty linked AND the quick-terminal (quake drop-down + hotkey).
#
# Usage:
#   bash scripts/build.sh [extra cargo args]
#
# Then install with:
#   sudo bash scripts/install.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REL="$REPO_ROOT/target/release"
cd "$REPO_ROOT"

cargo build --release --features jmux/link-ghostty,jmux/quick-terminal "$@"

for bin in jmux-app jmux-cli; do
  if [[ ! -x "$REL/$bin" ]]; then
    echo "error: $REL/$bin missing after build" >&2
    exit 1
  fi
done

# Same check install.sh makes: the portal symbol proves quick-terminal is in.
if ! grep -aqF 'org.freedesktop.portal.GlobalShortcuts' "$REL/jmux-app"; then
  echo "error: quick-terminal missing from $REL/jmux-app" >&2
  exit 1
fi

echo "Built $REL/jmux-app and $REL/jmux-cli (ghostty + quick-terminal)."
echo "Install with: sudo bash scripts/install.sh"
