# This fork: LightCraft plus techneissa's own features

The branch **`custom`** is the official LightCraft ([storytold/lightcraft](https://github.com/storytold/lightcraft))
plus the features listed below. The installed app is built from `custom`.

## Update the app

1. Click **Update LightCraft** in Applications. (Same thing in Terminal:
   `~/Documents/workspace/gitClones/update-lightcraft.sh`; `update-craft-apps.sh` updates every ArtCraft app.)
2. It gets the official update, merges it into `custom`, builds and installs the app (about 5 to 10 minutes), and
   backs `custom` up to this fork.
3. If it stops with "the update touches the same code as your edits", nothing was changed. Ask an agent to
   "update LightCraft" (it follows the `craftupdate` skill).

The script is `update-craft.sh` in the same folder, shared by every ArtCraft app; it keeps your edits in any
app whose repo has a `custom` branch. The official release download (`update-lightcraft-release.sh`) installs
the official app without these features.

## Where things are

| Name | What it is |
|---|---|
| `origin` | the official repo, where updates come from (never pushed to) |
| `mine` | this fork, [techneissa/lightcraft](https://github.com/techneissa/lightcraft), the backup |
| `main` | a plain copy of the official `main`, kept in step by the update script |
| `custom` | the official code plus every feature below: the app is built from it |
| `feat/…`, `fix/…` | one feature each, based on the official code, kept for reference |

Local folder: `~/Documents/workspace/gitClones/lightcraft`. Git remembers how a merge conflict was fixed
(`rerere`) and fixes it the same way the next time.

## Add a feature

1. `git fetch origin && git switch -c feat/<name> origin/main`
2. Put the code in new files; touch existing files only with small hooks (a `mod` line, a menu entry, a match arm,
   a state field). Never reformat, rename or move official code. This is what keeps updates conflict-free.
3. Add tests, then `cargo xtask ci --quick`.
4. `git switch custom && git merge --no-ff feat/<name>`, then `git push mine custom feat/<name>`.
5. Add a row to the table below, and run Update LightCraft to install it.

## The features

| Feature | Branch | Own files | Hooks in official files | Tests |
|---|---|---|---|---|
| Export Animated GIF (File menu), quality above ffmpeg | `feat/animated-gif-export` | `crates/codecs/src/animated_gif.rs` and `animated_gif/quantize.rs`, `crates/engine/src/animated_gif.rs`, `crates/ui-egui/src/dialogs/animated_gif.rs` | engine `lib.rs` (module, session field), `cmd/export.rs` (two commands), `slideshow_export.rs` (helpers made `pub(crate)`); UI dialog, menu and frame-loop hooks; `Cargo.toml` (`gif`, `color_quant`) | `animated_gif` in codecs, engine and ui-egui |
| Numbers 1, 2, 3 on selected photos (the GIF's order) | `feat/animated-gif-export` | `crates/ui-egui/src/panels/selection_order.rs` | `panels.rs`, `panels/grid.rs` (3 lines) | `selection_order` |
| Draggable Before/After split line, Split button | `feat/draggable-split` | `crates/ui-egui/src/panels/split_divider.rs` | `panels.rs`, `state.rs`, `region.rs`, `panels/detail.rs`, `icons.rs`, `panels/bottombar.rs` | `split_divider`, `before_after` |
| Command Palette on ⌘P (Print has no shortcut) | `feat/command-palette` | `crates/ui-egui/src/panels/command_palette.rs` and its `tests.rs` | `panels.rs`, `state.rs`, `menus/run.rs`, `menus/ui_commands.rs`, `shortcuts.rs`, `app/frame.rs` | `command_palette` |
| Edit All N Selected (Auto Sync on by default; AI Denoise follows it) | `fix/edit-all-selected` | `crates/ui-egui/src/panels/edit_all.rs` | engine `cmd/denoise.rs`, `lib.rs`, `library.rs`, `cmd/develop.rs`; UI `app.rs`, `panels.rs`, `panels/edit.rs` | `edit_all`, `denoise`, `prefs` |

Every feature also has an entry in `docs/whats-new.md` and a row in `docs/parity-checklist.md`; clashes there are
fixed by the update script (both sides kept).

The tests of every feature, after a merge:

```bash
cargo test -p lightcraft-codecs --lib animated_gif
cargo test -p lightcraft-engine --lib -- animated_gif denoise prefs
cargo test -p lightcraft-ui-egui --lib -- animated_gif selection_order split_divider before_after command_palette edit_all
```

## Known failures on this Mac (not caused by the fork)

- `tests::titlebar::double_click_on_the_empty_bar_zooms_then_restores`: reads the macOS setting for double-clicking
  a title bar, which is off here.
- `accessibility::every_interactive_node_has_a_name_in_every_view_and_tool`: fails on official `main` too (a "▸" the
  test fonts can't draw).
