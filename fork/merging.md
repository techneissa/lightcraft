# Merging feature branches without clashes

Two branches that each add a feature touch no common file, so GitHub's merge button never reports a conflict
between them (techneissa, 2026-11-01: five branches in one evening made each other conflict about six times, always
in the same shared notes).

| What used to be shared | Now |
|---|---|
| the feature list in `FORK.md` | one `List:` line in the feature's own `fork/<slug>.md`; `fork/features.sh` prints every line; the older ones are in `fork/features.md`, which new features never edit |
| long `run cargo test …` lines in `fork/tests.sh` | `fork/tests.d/<slug>.txt`, one file per feature; `tests.sh` loops over them |
| the feature's entry in `docs/whats-new.md` | `## What's New entry` in `fork/<slug>.md`; `fork/sync-docs.py` copies it into `docs/whats-new.md` after the merge |
| the feature's rows in `docs/parity-checklist.md` | `## Parity rows` in `fork/<slug>.md`, copied the same way |
| the parity summary table (`cargo xtask parity --write`) | not committed in a feature branch; `fork/sync-docs.py` refreshes it after the merge. A stale summary only prints a note in `cargo xtask parity`, never fails it |
| `docs/gaps.md` | not edited in a feature branch, except one existing row changed in place |

The sync is one commit on `custom` by whoever merges (`python3 fork/sync-docs.py`, then `docs: sync fork notes`). It is
safe to run twice. Nothing else touches those two official files, so an update from the official project merges into
them as before.

## Why separate files and not `merge=union`

A `.gitattributes` line `merge=union` keeps both sides of a clash in git on this Mac, but GitHub's merge button and its
"This branch has conflicts" check ignore `.gitattributes` merge settings (reported by GitHub support, and still so in the
February 2021 test in the keep-a-changelog thread; nothing newer found 2026-11-02). Branches that only clash on
GitHub would stay blocked, so the fix is that they never touch the same lines. `merge=union` would also keep both
copies of a line that both sides changed (the "Last reviewed" line of the parity doc) and so break the update
workflow's own handling of that file (`fork-build.yml`). It is not used.

## If two branches still clash

- Merge `mine/custom` into your branch (`git merge mine/custom`), fix, push. Git remembers the fix (`rerere`).
- In a feature branch, the clash is in a file you were told not to edit: undo your edit to it and move the text into
  your `fork/<slug>.md` as above.
- A clash in official code is a real one: both hooks are kept, as before.
