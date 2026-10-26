---
name: changelog
description: Write the changelog entry for the changes since the last release. Use when the user asks for a changelog entry, release notes for CHANGELOG.md, or "what changed since the last tag".
argument-hint: "[version]"
---

# Changelog entry

1. Run `sh "$SKILL_DIR/scripts/commits.sh"` to list the commits since the last tag.
2. Read [the format](references/format.md) and group the changes the way it says.
3. Add the entry for version $ARGUMENTS (or "Unreleased") at the top of CHANGELOG.md, creating the file if there is none.

Describe what changed for users, not how the code changed. Leave out merges and changes to CI.
