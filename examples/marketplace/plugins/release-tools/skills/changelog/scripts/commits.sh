#!/bin/sh
# Commits since the last tag, oldest first, one line each.
set -e
last=$(git describe --tags --abbrev=0 2>/dev/null || true)
if [ -n "$last" ]; then
  git log --reverse --format='%h %s' "$last..HEAD"
else
  git log --reverse --format='%h %s'
fi
