#!/usr/bin/env bash
#
# Publish the Markdown pages in ./wiki/ to this repo's GitHub Wiki.
#
# GitHub does NOT expose an API for wiki content, and it only creates the
# `*.wiki.git` repository after the first wiki page is created in the browser.
# So this is a one-time manual unlock, then a one-command sync forever after:
#
#   1. Create the first wiki page once (any content — it gets overwritten):
#        https://github.com/ENC4YP7ED/squrust/wiki/_new
#   2. Run this script from the repo root:
#        ./scripts/publish-wiki.sh
#
# Requires: git, and either the `gh` CLI authenticated or git credentials for
# github.com.
set -euo pipefail

WIKI_REMOTE="https://github.com/ENC4YP7ED/squrust.wiki.git"
SRC_DIR="$(cd "$(dirname "$0")/.." && pwd)/wiki"

# Use a gh token if available so HTTPS push is non-interactive.
if command -v gh >/dev/null 2>&1 && gh auth token >/dev/null 2>&1; then
    TOKEN="$(gh auth token)"
    WIKI_REMOTE="https://x-access-token:${TOKEN}@github.com/ENC4YP7ED/squrust.wiki.git"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if ! git clone --quiet "$WIKI_REMOTE" "$TMP" 2>/dev/null; then
    echo "error: could not clone the wiki repo." >&2
    echo "Create the first page once in the browser, then re-run:" >&2
    echo "  https://github.com/ENC4YP7ED/squrust/wiki/_new" >&2
    exit 1
fi

cp "$SRC_DIR"/*.md "$TMP"/
cd "$TMP"
git add -A
if git diff --cached --quiet; then
    echo "Wiki already up to date."
    exit 0
fi
git -c user.name="ENC4YP7ED" -c user.email="ENC4YP7ED@proton.me" \
    commit --quiet -m "Sync wiki from repo wiki/"
git push --quiet
echo "Wiki published: https://github.com/ENC4YP7ED/squrust/wiki"
