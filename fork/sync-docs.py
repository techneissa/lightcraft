#!/usr/bin/env python3
"""Copies the fork's notes for the official docs into them, after a feature branch is merged.

A feature branch never edits docs/whats-new.md or docs/parity-checklist.md (official files that every
branch and every upstream update touches: two branches adding to them clash). It keeps its text in its
own fork/<slug>.md instead, under two optional headings:

  ## What's New entry     the `### Title` block for docs/whats-new.md
  ## Parity rows          table rows for docs/parity-checklist.md (`| LR-REM-DETECT | … |`)

Run after merging a branch into custom (once, whoever merges):  python3 fork/sync-docs.py
It adds what is missing and leaves what is there alone, so running it twice changes nothing:
- an entry whose `### Title` line is not in docs/whats-new.md goes first under today's `## YYYY-MM-DD` heading
  (made when the newest heading is another day);
- a row whose id is not in docs/parity-checklist.md goes after the last row of its section (`LR-REM-` → the
  rows starting `| LR-REM-`), then `cargo xtask parity --write` refreshes the summary table.
Commit the result on custom as "docs: sync fork notes". `--no-summary` skips the cargo step.
"""
import datetime
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WHATS_NEW = ROOT / "docs" / "whats-new.md"
PARITY = ROOT / "docs" / "parity-checklist.md"
ENTRY_HEADING = "## What's New entry"
ROWS_HEADING = "## Parity rows"


def section(text, heading):
    """The lines under `heading` up to the next `## ` heading, without trailing blank lines."""
    lines = text.split("\n")
    if heading not in lines:
        return []
    out = []
    for line in lines[lines.index(heading) + 1 :]:
        if line.startswith("## "):
            break
        out.append(line)
    while out and not out[-1].strip():
        out.pop()
    while out and not out[0].strip():
        out.pop(0)
    return out


def row_id(row):
    cells = row.split("|")
    return cells[1].strip() if len(cells) > 2 else ""


def add_entries(entries):
    """Insert the entries missing from docs/whats-new.md; returns the titles added."""
    text = WHATS_NEW.read_text()
    lines = text.split("\n")
    present = {line.strip() for line in lines}
    new = [e for e in entries if e[0].strip() not in present]
    if not new:
        return []
    today = datetime.date.today().isoformat()
    block = "\n\n".join("\n".join(e) for e in new) + "\n"
    first = next((i for i, line in enumerate(lines) if re.match(r"## \d{4}-\d\d-\d\d\s*$", line)), None)
    if first is None:
        sys.exit("docs/whats-new.md has no `## YYYY-MM-DD` heading")
    if lines[first].strip() == "## " + today:
        # after the heading and its blank line; the block's last blank line keeps it apart from the next entry
        lines[first + 2 : first + 2] = block.split("\n")
    else:
        lines[first:first] = ["## " + today, ""] + block.split("\n")
    WHATS_NEW.write_text("\n".join(lines))
    return [e[0] for e in new]


def add_rows(rows):
    """Insert the rows whose id is missing from docs/parity-checklist.md; returns the ids added."""
    lines = PARITY.read_text().split("\n")
    added = []
    for row in rows:
        rid = row_id(row)
        if not rid or any(row_id(line) == rid for line in lines if line.startswith("|")):
            continue
        m = re.match(r"(LR-[A-Z0-9]+-)", rid)
        if not m:
            sys.exit(f"parity row id `{rid}` does not look like LR-<SECTION>-<NAME>")
        last = max((i for i, line in enumerate(lines) if line.startswith("| " + m.group(1))), default=None)
        if last is None:
            sys.exit(f"parity row `{rid}`: no existing section has rows starting `| {m.group(1)}`")
        lines.insert(last + 1, row)
        added.append(rid)
    if added:
        PARITY.write_text("\n".join(lines))
    return added


def main():
    entries, rows = [], []
    for f in sorted((ROOT / "fork").glob("*.md")):
        text = f.read_text()
        entry = section(text, ENTRY_HEADING)
        if entry:
            if not entry[0].startswith("### "):
                sys.exit(f"{f.name}: `{ENTRY_HEADING}` must start with a `### Title` line")
            entries.append(entry)
        rows += [line for line in section(text, ROWS_HEADING) if line.startswith("| LR-")]
    titles = add_entries(entries)
    ids = add_rows(rows)
    for t in titles:
        print("whats-new: added", t)
    for i in ids:
        print("parity: added", i)
    if not titles and not ids:
        print("docs already in sync")
    elif ids and "--no-summary" not in sys.argv:
        subprocess.run(["cargo", "xtask", "parity", "--write"], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
