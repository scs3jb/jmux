#!/bin/bash
# Capture the static panel screenshots headlessly:
#   docs/screenshots/{markdown,diff,project,notes,browser}.png
# These are content views (no animation needed), so it's plain grim — no virtual
# pointer/keyboard. Needs the demo sandbox from docs/autocapture.sh (theme + dirs).
set -e
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SB=/tmp/cmux-demo; W="$SB/web"

# ---- synthetic content ----------------------------------------------------------
mkdir -p "$W/src/api" "$W/tests"
cat > "$W/README.md" <<'MD'
# webapp

A small service that issues and validates JWT sessions.

## Features
- `/api/login` — issue a signed token
- `/api/users` — list users (auth required)
- Rate limiting on all write endpoints

## Quick start

```bash
npm install
npm run dev      # http://localhost:3000
```

## Architecture
The auth module lives in `src/auth.rs`; request handlers in `src/api/`.

> Note: tokens expire after 24h.
MD
printf '[package]\nname = "webapp"\nversion = "0.3.1"\nedition = "2021"\n' > "$W/Cargo.toml"
printf '{ "name": "webapp", "version": "0.3.1" }\n' > "$W/package.json"
printf 'fn main() { webapp::serve(); }\n' > "$W/src/main.rs"
printf 'pub mod api;\npub fn serve() {}\n' > "$W/src/lib.rs"
printf 'pub mod users;\n' > "$W/src/api/mod.rs"
printf 'pub fn list() -> Vec<String> { vec![] }\n' > "$W/src/api/users.rs"
printf '#[test]\nfn validates_jwt() { assert!(true); }\n' > "$W/tests/auth_test.rs"
# ---- notes: scope-grouped scratchpads (Global = blue, Folder = green) ----------
# The notes panel reads HOME=$SB; populate the mirror tree it scans. The Folder
# scope is keyed to the git repo root of the workspace cwd ($W).
ND="$SB/.local/share/cmux/notes"
mkdir -p "$ND/global" "$ND/local${W}"
cat > "$ND/global/architecture.md" <<'NOTES'
# Architecture

- auth module → src/auth.rs (JWT issue/validate)
- request handlers → src/api/
- tokens expire after 24h
NOTES
cat > "$ND/global/conventions.md" <<'NOTES'
# Conventions

- conventional commits
- rustfmt for Rust, 2-space indent in JS
NOTES
cat > "$ND/local${W}/auth-plan.md" <<'NOTES'
# Auth plan

- [x] JWT validation
- [ ] refresh tokens
- [ ] rate-limit /api/login
NOTES
cat > "$ND/local${W}/todo.md" <<'NOTES'
# webapp — TODO

## In progress
- [ ] rate-limit /api/login
- [ ] integration tests for the feed

## Ideas
- cache user lookups (frecency?)
- switch deploy to fly.io
NOTES
# a git repo with an unstaged change for the diff viewer
( cd "$W"
  export GIT_CONFIG_GLOBAL="$SB/.gitconfig" GIT_CONFIG_SYSTEM=/dev/null
  git config --global user.email dev@demo.local; git config --global user.name demo
  rm -rf .git; git init -q
  printf 'use jsonwebtoken::{decode, Validation};\n\npub fn validate(token: &str) -> bool {\n    !token.is_empty()\n}\n' > src/auth.rs
  git add -A; git commit -qm initial
  printf 'use jsonwebtoken::{decode, encode, Header, Validation};\n\npub fn validate(token: &str) -> Result<Claims, AuthError> {\n    let key = DecodingKey::from_secret(SECRET);\n    decode::<Claims>(token, &key, &Validation::default()).map(|d| d.claims).map_err(|_| AuthError::Invalid)\n}\n\npub fn issue(user_id: u64) -> String {\n    let claims = Claims { sub: user_id, exp: now() + 86_400 };\n    encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET)).unwrap()\n}\n' > src/auth.rs )
# index.html for the browser is committed alongside this script (browser-page.html); copy it in
cp "$HERE/browser-page.html" "$W/index.html"

# ---- capture --------------------------------------------------------------------
source "$HERE/harness.sh"
keep_selected(){ local ids; ids=$(R list 2>/dev/null | python3 -c "import sys,json
for w in json.load(sys.stdin)['result']['workspaces']:
    if not w.get('is_selected'): print(w['id'])"); for id in $ids; do R close "$id" >/dev/null 2>&1; done; }

( cd "$W" && env -i PATH=/usr/bin python3 -m http.server 8137 --bind 127.0.0.1 ) >/tmp/httpd.log 2>&1 &
HTTPD=$!; sleep 1
start_sway
shot(){ local p="$1"; start_cmux
  case "$p" in
    markdown) stage_only docs-ws; R open "$W/README.md" >/dev/null 2>&1 ;;
    notes)    stage_only demo; R notes >/dev/null 2>&1 ;;
    browser)  stage_only docs-ws; R open http://localhost:8137/ >/dev/null 2>&1; sleep 2 ;;
    diff)     R diff "$W" >/dev/null 2>&1; sleep 2; keep_selected ;;
    project)  R project "$W" >/dev/null 2>&1; sleep 2; keep_selected ;;
  esac
  sleep 2.5; G "$REPO/docs/screenshots/$p.png" && echo "  wrote $p.png"
  kill $CP 2>/dev/null; sleep 0.8
}
for p in markdown diff project notes browser; do shot "$p"; done
stop; kill $HTTPD 2>/dev/null
echo "wrote 5 panel screenshots. Review each for leaks before committing."
