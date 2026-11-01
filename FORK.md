# This fork: LightCraft plus techneissa's own features

The branch **`custom`** is the official LightCraft ([storytold/lightcraft](https://github.com/storytold/lightcraft))
plus the features listed below. The installed app is built from `custom`.

**Mac only.** techneissa uses this fork on macOS and nowhere else (techneissa, 2026-11-01). Its features and fixes
need to work on the Mac only: no Windows, Linux or web (WASM) work, checks, screenshots or fixes for them. A Fork CI
failure that only Linux has (a macOS-only test, a Linux-only path) is not a blocker; list it under "Known failures".
The official code's own platforms are the official project's job.

## Update the app

Click **Update LightCraft** in Applications (or `~/scripts/craft/update-lightcraft.sh`). The steps, and what to do
when it stops: [fork/update.md](fork/update.md).

## GitHub builds the app

Fork build (`fork-build.yml`) and Fork CI (`fork-ci.yml`), the daily update agent, the switched-off official
workflows: [fork/github-builds.md](fork/github-builds.md).

## Where things are

Remotes (`origin` official, `mine` this fork), branches (`main`, `custom`, `feat/…`), the `custom-build` release and the
local folder: [fork/where-things-are.md](fork/where-things-are.md).

Known failures (on this Mac, and on Fork CI only): [fork/known-failures.md](fork/known-failures.md).

## Add a feature

A feature branch adds files and never edits a list two other branches also edit (techneissa, 2026-11-01: five
parallel branches made each other conflict about six times in one evening).

1. `git fetch origin && git switch -c feat/<name> origin/main`
2. Put the code in new files; touch existing files only with small hooks (a `mod` line, a menu entry, a match arm,
   a state field). Never reformat, rename or move official code. This is what keeps updates conflict-free.
   No help line under a setting, toggle or button whose label already says what it does; a needed one is one short
   sentence (techneissa, 2026-10-29: Settings had grown long with lines that repeated their labels).
3. Add tests, and their command in a new file `fork/tests.d/<slug>.txt` (one command per line). Check them as
   [fork/checks.md](fork/checks.md) says: on this Mac only the touched tests, clippy and fmt, never
   `cargo xtask ci` or `ci --quick`; the full suite runs on GitHub (Fork CI).
4. Write `fork/<slug>.md` with the headings What it does / Branch / Own files / Hooks in official files / Tests
   / When upstream merges (the last only for a feature that took an upstream PR early). Under the title, a line
   `List: <one-line summary>.` (end it with the branch name) is the feature's line in the list. For the official docs, two
   optional headings in the same file: `## What's New entry` (a `### Title` block) and `## Parity rows` (table
   rows `| LR-… |`).
5. Do **not** edit `fork/features.md`, `fork/tests.sh`, `docs/whats-new.md`, the parity summary or `FORK.md`
   in the branch. Leave `docs/parity-checklist.md` and `docs/gaps.md` alone too, except to change one existing
   row in place (a row you edit, not a row you add, and not the "Last reviewed" line).
6. Merge: `git switch custom && git merge --no-ff feat/<name>`, or the PR's merge button. Whoever merges then runs
   `python3 fork/sync-docs.py` once, commits `docs: sync fork notes` on `custom` and pushes it: the script copies the
   What's New entries and parity rows into the official docs and refreshes the parity summary. Then
   `git push mine custom feat/<name>` and run Update LightCraft to install it.

Why this works, and what to do when two PRs still clash: [fork/merging.md](fork/merging.md).

## The features

One line each, in [fork/features.md](fork/features.md) (older) and in each newer `fork/<slug>.md` (its `List:`
line). Print them all: `fork/features.sh`.

Every feature also has an entry in `docs/whats-new.md` and a row in `docs/parity-checklist.md`; a feature's own
text for them lives in its `fork/<slug>.md` until `fork/sync-docs.py` copies it.

The tests of every feature, after a merge: run `fork/tests.sh`.

To find which feature owns an official file: `grep -l '<path>' fork/*.md`.
