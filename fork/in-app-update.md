# In-app update check for this fork

## What it does

Help ▸ Check for Updates…, Settings ▸ General ▸ Updates ▸ Check for Updates and the check at start ask GitHub for this fork's own build, never for the official LightCraft release (which would install the app without the fork's features). The official check no longer runs in this build.

Statuses, one line in Settings and in the Help window: **Up to Date**, **A New Build Is Ready** (with an Update button), **A New Build Is Being Made** (the release says `commit: updating`), **Unknown Build** (the app's commit is not known; Update still works), or a short error ("Can't Reach GitHub", "GitHub Is Busy, Try Later"). The check at start says nothing unless a build is ready; then it opens the window.

Update opens the **Update LightCraft** app (`open -a "Update LightCraft"`). That app runs `~/scripts/craft/update-lightcraft.sh` (`update-craft.sh`), which downloads the build, quits LightCraft, installs it and, at the end, opens LightCraft again (`tell application "LightCraft" to activate` in `update-lightcraft.applescript`). Nothing in `~/scripts/craft/` changed for this feature. Running `update-lightcraft.sh` alone in Terminal does not reopen the app; the button does.

## How it decides

1. The release: `GET https://api.github.com/repos/techneissa/lightcraft/releases/tags/custom-build`. Its notes are `commit: <sha>`, `commit: updating` while a build uploads (workflow `fork-build.yml`), or `commit: none`.
2. The app's commit: the `LightCraftBuildCommit` key of `LightCraft.app/Contents/Info.plist`. `packaging/macos/package.sh` writes it (from `LIGHTCRAFT_BUILD_SHA`, which `packaging/env.sh` sets to `git rev-parse HEAD`) for every build: GitHub's `fork-build.yml` and a build on the Mac by `update-craft.sh` both run that script, so both carry the commit they were built from. The update script's stamp `~/Library/Caches/craft-update/lightcraft.commit` holds the same commit after an install; the app does not read it (it says what the last install was, not what is running). A run outside a packaged app (`cargo run`) has no Info.plist: Unknown Build.
3. Same commit: Up to Date, no second request. Otherwise `GET .../compare/<app>...<release>?per_page=1`: `status: ahead` is newer; `identical`, `behind` and `diverged` are Up to Date. A 404 or 422 (GitHub does not know the app's commit, for example a Mac build of work not pushed) is Unknown Build.

## Details

- The request runs on a worker thread with a row in the activity stack (kind `update`, "Checking for updates"), through `lightcraft-fetch`'s HTTPS client with timeouts; an error never panics.
- The official `craft_update::Updater` stays in `updates_ui.rs` but nothing calls its check; `UPDATE_APP` in `lib.rs` is left as it was.
- Tests never touch the network: the real getter is replaced by a refusing one under `cfg(test)`, and tests install fakes.

## Branch

`feat/fork-update-check`

## Own files

`crates/ui-egui/src/fork_update.rs` (the logic, the state, the Settings row, the window, unit tests), `crates/ui-egui/src/tests/fork_update.rs`

## Hooks in official files

`dialogs/updates_ui.rs` (a `fork` field; `tick` asks the fork instead of the official check; `show` draws the fork's window; `blocks_input`), `dialogs/settings/general.rs` (one call), `lib.rs` (`mod fork_update;`), `tests.rs` (`mod fork_update;`), `crates/ui-egui/Cargo.toml` (`lightcraft-fetch`).

## Tests

`cargo test -p lightcraft-ui-egui --lib fork_update`
