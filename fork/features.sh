#!/usr/bin/env bash
# Prints the fork's feature list: the older features from fork/features.md, then every feature whose
# fork/<slug>.md has a `List:` line under its title. From any folder: fork/features.sh
set -u
cd "$(dirname "$0")"
sed -n '/^- \[/p' features.md
for f in *.md; do
  list="$(sed -n 's/^List: //p' "$f" | sed -n 1p)"
  [ -n "$list" ] || continue
  title="$(sed -n 's/^# //p' "$f" | sed -n 1p)"
  echo "- [$title]($f): $list"
done
