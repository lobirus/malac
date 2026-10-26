#!/bin/sh
# PreToolUse hook: refuse `git push --force`. The tool call arrives as JSON on stdin;
# exit code 2 blocks it and stderr tells the agent why.
if grep -Eq 'git push[^"]*(--force|-f)( |"|$)'; then
  echo "Force pushes are blocked by release-tools; push normally or ask the user." >&2
  exit 2
fi
exit 0
