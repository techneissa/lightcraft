# Settings sync

List: Settings ▸ Sync moves all settings to another Mac in one step: Export and Import Settings…, or Save to Folder and Load from Folder with Keep in Sync, in a folder both ArtCraft apps share (issue #59). Branch `feat/59-settings-sync`.

## What it does

Settings ▸ Sync (a new tab) has:
- **Sync Folder:** the chosen folder's path and Choose…; a Mac with none chosen shows the folder the other ArtCraft apps use (read from the shared file) with Use This Folder. Save to Folder, Load from Folder and Keep in Sync wait for a folder.
- **Settings File:** Export Settings… and Import Settings…, the same as one `.lcsettings` file, in a save or open window.
- **Included:** User Presets (on by default; a setting of this Mac).

**The file** is JSON: `kind` `lightcraft-settings`, `version` 1, `app`, and two parts, each optional on import. `ui` is `ui.json` minus the keys of one Mac; `presets` is the user presets as a `.lcpreset` file (merged on import, an identical preset is not added twice; built-in presets are never in it; favourite marks are not kept). A save adds `savedBy` (`{"computer": "<the Mac's name as Sharing shows it>", "at": "<RFC 3339 with the Mac's offset, to the second>"}`), also in an exported file: a note about the file, never a setting; missing or unreadable is ignored.

**What is left out** (the deny list, `settings_sync/rules.rs`, `LEFT_OUT` and `LEFT_OUT_SETTINGS`, each with its reason): window size and place, panel positions (`docking`), side bar widths, zoom and pan, the view and panels open now, grid thumbnail sizes (they follow the screen), the tool and tab in use, soft proofing and printer profiles, local folders (`localRoots`, `hiddenLocations`), the loupe overlay's file, `secondWindow`, `sync` (this Mac's folder, Keep in Sync, last saved or loaded, Include User Presets), and in `settings`: library paths, the external editor and PhotoCraft paths, GPU, window system, preview limit and memory, the lens database folder, the monitor profile. Logs and AI models are not in `ui.json` at all. Everything else in `ui.json` goes in: appearance and language, keyboard shortcuts (the whole keymap: both slots, chords, When), the Edit panel sections' order and hidden ones, the left side bar sections, the photo right-click menu and its presets, top bar items, folded sections, brush, Remove and crop defaults, overlays, Visualize options, grid and filmstrip options.

**A new setting syncs without a change here** (the file is built from the deny list). `every_ui_key_is_decided` fails until the new `ui.json` key is named in `SYNCED` / `SYNCED_SETTINGS` or `LEFT_OUT` / `LEFT_OUT_SETTINGS`, so nobody forgets to decide. A key that stays empty at its default (`window`, `showClipping`, `photoMenu`, `photoMenuPresets`, `topBarHidden`) isn't seen by that test; the file writes the last three explicitly so a Mac that went back to the default arrangement takes the other back too.

**Import** (`settings_sync/plan.rs`) reads the whole file first and changes nothing until OK:
- A setting, section or part this build doesn't have (a newer LightCraft; a left-out key) is skipped and named (`ui.key`, `ui.settings.key`, a part's name; ids that this build dropped from the sidebar, Edit panel sections, top bar items, photo menu and keymap are named too). The prompt says so in one line: "3 settings from a newer LightCraft were skipped: …", the first five, then "and N more", the full list on hover.
- A part or key the file lacks leaves that setting as it is.
- A bad value for a setting this build knows (wrong type, unknown choice, number out of range, a shortcut that isn't one, a condition that doesn't parse) is a short error that names it, and nothing changes. In a file with a newer `version` it is skipped with a note instead.
- The prompt names who saved the file and when ("Saved on Studio Mac, November 2, 2026 at 14:30", local time), what it replaces and how many presets it adds.
- OK replaces the named settings in one step (the way `ui.set` does) and keeps what the file never holds: work in progress such as copies open in an editor, the status line, the compare and reference photos.

**The shared contract** (the same as PhotoCraft's `fork/settings-sync.md`): `~/Library/Application Support/ArtCraft/sync-folder.json` = `{"path": "<absolute folder>"}` (readers ignore other keys and take a missing, bad or relative path as nothing to offer; writers keep other keys, write atomically and replace a damaged file); the file is `lightcraft.lcsettings` in the folder (PhotoCraft's is `photocraft.pcsettings`; the other app's file is never read or written); writes go to a hidden temporary name in the same folder and are renamed over the old file, and a failed write leaves the old file whole.

**Keep in Sync** (per Mac, never in the file): on quit the settings are saved to the folder, unless the folder's file is newer and from another Mac (by computer name) than the last one this Mac saved or loaded (`ui.sync.last`, the `savedBy.at`; empty: any file from another Mac). At start such a file is offered as the load prompt, once no other dialog is open; Cancel offers it again at the next start. A folder that is gone is a message in the status bar at start and is logged on quit. The save runs in `on_exit`, before `ui.json` is written, so the time of the save is in it.

**Commands** (no menu item; the buttons run them, and the Command Palette lists them):
- `settings.export {path?}`, `settings.import {path?, text?, dryRun?, prompt?}`, `settings.syncSave`, `settings.syncLoad {dryRun?, prompt?}`, `settings.syncChoose {path?}`.
- By a person without parameters: Export and Import ask for the file (`pick`), Save writes at once, Load shows the prompt, Choose asks for a folder. With parameters the command does the work without the prompt (`prompt: true` shows it; the file dialogs' answers come back that way).
- **Agents** (the control channel and MCP): no command takes a path; `settings.export` replies `{document}` instead of writing a file, `settings.import` takes `text` and applies at once (`dryRun` only reports), `settings.syncSave` / `syncLoad` use the folder the user chose, and `settings.syncChoose` is refused. `ui.set` refuses the `sync` key, so an agent can't change the folder, Keep in Sync or the last-saved time.
- The reply of an import (also with `dryRun`) has `applied`, `parts`, `settings`, `presets`, `skipped` (notes), `unknown` (the names, at most 200) and `unknownCount`.

Desktop only (macOS in this fork): the engine module and the commands are not built for the web; the Sync tab says so there.

## Branch

`feat/59-settings-sync`

## Own files

engine `crates/engine/src/settings_folder.rs` + `settings_folder/tests.rs`; ui-egui `src/settings_sync.rs` + `settings_sync/` (`rules.rs`, `plan.rs`, `prompt.rs`, `run.rs`, `tests.rs`, `page_tests.rs`), `src/dialogs/settings/sync.rs`; `fork/settings-sync.md`, `fork/tests.d/settings-sync.txt`

## Hooks in official files

engine `Cargo.toml` (`chrono` with `clock`, for the Mac's time zone) and `lib.rs` (`mod settings_folder`); ui-egui `lib.rs` (`mod settings_sync`, the field `sync_settings`), `app.rs` (the field), `app/frame.rs` (`offer_at_start`), `state.rs` (the field `sync` and its default), `state/dialog.rs` (`Dialog::SettingsImport`), `dialogs.rs` (title, OK label, body), `dialogs/confirm.rs` (OK), `dialogs/settings.rs` (the `sync` tab), `menus/ui_commands.rs` + `menus/run.rs` (five commands), `control.rs` (`ui.set` refuses `sync`); `apps/lightcraft/src/main.rs` (`save_on_quit` in `on_exit`).

## Tests

engine `settings_folder`; ui-egui `settings_sync`

```bash
cargo test -p lightcraft-engine --lib -- settings_folder
cargo test -p lightcraft-ui-egui --lib -- settings_sync
```

## What's New entry

### Settings Sync
- Settings ▸ Sync moves all your settings to another Mac: Export Settings… and Import Settings… use one file, or Save to Folder and Load from Folder use a folder you choose, such as one in iCloud Drive. Keep in Sync saves there when you quit and offers a newer file from another Mac when you start.
- Appearance, keyboard shortcuts, arrangements, tool defaults and your presets go along; window sizes, panel positions and folders of one Mac stay. Loading names when and on which Mac the file was saved, and asks first.
- PhotoCraft uses the same folder, with its own file.

## Parity rows

| LR-PREF-SETTINGSSYNC | Settings sync | — | ✅ | `cmd:settings.export`, `cmd:settings.import`, `cmd:settings.syncSave`, `cmd:settings.syncLoad`, `cmd:settings.syncChoose`, `crates/ui-egui/src/settings_sync.rs`, `crates/engine/src/settings_folder.rs`, `crates/ui-egui/src/dialogs/settings/sync.rs` | an extra (the reference app has no settings export): Settings ▸ Sync saves and loads all settings (appearance, keyboard shortcuts, arrangements, tool defaults, user presets) as one `.lcsettings` file or in a chosen folder shared with PhotoCraft; Keep in Sync saves on quit and offers a newer file from another Mac at start; a file from a newer LightCraft imports what this build knows and names the rest; a bad value or a damaged file changes nothing; agents give no path and can't change the folder. See `fork/settings-sync.md` |
