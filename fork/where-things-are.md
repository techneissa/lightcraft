# Where things are

| Name | What it is |
|---|---|
| `origin` | the official repo, where updates come from (never pushed to) |
| `mine` | this fork, [techneissa/lightcraft](https://github.com/techneissa/lightcraft), the backup |
| `main` | a plain copy of the official `main`, kept in step by the update script |
| `custom` | the official code plus every feature below: the app is built from it (the default branch here). GitHub blocks force pushes to it and its deletion (ruleset "Protect custom and main", also on `main`, techneissa 2026-11-01) |
| `custom-build` | the release holding the app GitHub built from `custom` |
| `feat/…`, `fix/…` | one feature each, based on the official code, kept for reference |

Local folder: `$WORKSPACE/repos/lightcraft` (usually `~/Documents/workspace/repos/lightcraft`). Git remembers how a merge conflict was fixed
(`rerere`) and fixes it the same way the next time.
