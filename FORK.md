# This fork: LightCraft plus techneissa's own features

The branch **`custom`** is the official LightCraft ([storytold/lightcraft](https://github.com/storytold/lightcraft))
plus the features listed below. The installed app is built from `custom`.

## Update the app

1. Click **Update LightCraft** in Applications. (Same thing in Terminal:
   `~/Documents/workspace/gitClones/update-lightcraft.sh`; `update-craft-apps.sh` updates every ArtCraft app.)
2. It downloads the newest app GitHub built (below; seconds), when that build has everything the Mac's `custom`
   has. Official changes made since that build come with the next one (daily at 08:17, and after every push).
3. Otherwise, or with `update-lightcraft.sh --latest`, it gets the official update, merges it into `custom` and
   backs `custom` up to this fork. Then it installs the app: a download of seconds when GitHub has already built
   that version, else a build on the Mac (1 to 10 minutes).
4. If it stops with "the update touches the same code as your edits", nothing was changed. Ask an agent to
   "update LightCraft" (it follows the `craftupdate` skill).

The script is `update-craft.sh` in the same folder, shared by every ArtCraft app; it keeps your edits in any
app whose repo has a `custom` branch. The official release download (`update-lightcraft-release.sh`) installs
the official app without these features.

## GitHub builds the app

The workflow `.github/workflows/fork-build.yml` (Actions ▸ Fork build) runs daily, on every push to `custom`, and
by hand. It merges the official update into `custom` the same way the script does, builds the Mac app, and
publishes it as the release [`custom-build`](https://github.com/techneissa/lightcraft/releases/tag/custom-build)
(its notes name the commit). The update script downloads it (above). A run that hits a merge clash fails and
changes nothing; GitHub emails the failure. The run pushes its merge with the secret `FORK_PUSH_TOKEN`, the
fine-grained token "BuildCraft forks" (the LightCraft, PdfCraft and SolidCraft forks; Contents and Workflows:
read and write): GitHub's own token may not push an official update that changes a workflow file. Without the
secret it uses GitHub's own token.

A daily agent (the T3 thread "Daily LightCraft Update", Grok 4.5 on Cursor, 09:00) runs the update without
installing; when GitHub's run stopped on a clash, it merges by hand, runs the feature tests and pushes, and GitHub
builds again. It follows the "Daily run" section of the `craftupdate` skill. To stop it: switch it off under
Scheduled Tasks in T3, or ask any agent to "turn off the daily LightCraft update".

The official project's own workflows (CI, Nix, Packaging lint, SonarCloud) are switched off in this fork:
the official project runs them. A workflow the official project adds later starts switched on; switch it off
in Actions unless it is wanted here.

## Where things are

| Name | What it is |
|---|---|
| `origin` | the official repo, where updates come from (never pushed to) |
| `mine` | this fork, [techneissa/lightcraft](https://github.com/techneissa/lightcraft), the backup |
| `main` | a plain copy of the official `main`, kept in step by the update script |
| `custom` | the official code plus every feature below: the app is built from it (the default branch here) |
| `custom-build` | the release holding the app GitHub built from `custom` |
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
| Numbers 1, 2, 3 on selected photos (the GIF's order) | `feat/animated-gif-export` | `crates/ui-egui/src/panels/selection_order.rs` | `panels.rs`, `panels/grid.rs` (3 lines); engine `view.rs` (Shift-click adds the range from the anchor outward) | `selection_order` |
| Draggable Before/After split line, Split button | `feat/draggable-split` | `crates/ui-egui/src/panels/split_divider.rs` | `panels.rs`, `state.rs`, `region.rs`, `panels/detail.rs`, `icons.rs`, `panels/bottombar.rs` | `split_divider`, `before_after` |
| Command Palette on ⌘P (Print has no shortcut) | `feat/command-palette` | `crates/ui-egui/src/panels/command_palette.rs` and its `tests.rs` | `panels.rs`, `state.rs`, `menus/run.rs`, `menus/ui_commands.rs`, `shortcuts.rs`, `app/frame.rs` | `command_palette` |
| Edit All N Selected (Auto Sync on by default; AI Denoise follows it) | `fix/edit-all-selected` | `crates/ui-egui/src/panels/edit_all.rs` | engine `cmd/denoise.rs`, `lib.rs`, `library.rs`, `cmd/develop.rs`; UI `app.rs`, `panels.rs`, `panels/edit.rs` | `edit_all`, `denoise`, `prefs` |
| Taller, resizable open and save windows on macOS (at least 520 pt) | `feat/larger-file-dialogs` | `crates/filepanel` (the second crate allowed `unsafe`) | `apps/lightcraft/src/platform.rs` (1 call in `file_dialog`), app and workspace `Cargo.toml`, `xtask/src/layers.rs`, `AGENTS.md` / `CLAUDE.md` (the `unsafe` line) | `lightcraft-filepanel` |
| Arrange the photo menu (Settings ▸ Photo Menu), Before/After Split in it, one photo hides what needs several | `feat/photo-menu` (on `feat/animated-gif-export`) | `crates/ui-egui/src/panels/photo_menu.rs` and `photo_menu/` (`items.rs`, `editor.rs`, `tests.rs`) | `panels.rs` (module), `state/settings.rs` (`photo_menu` field), `dialogs/settings.rs` (tab), `menus/run.rs` + `menus/ui_commands.rs` (`photoMenu.*`), one call each in `panels/grid.rs`, `detail.rs`, `detail/filmstrip.rs`, `compare.rs` (2); `grid::context_menu` stays as the reference | `photo_menu` (its test fails when an update adds an item to the official menu: add it to `items::BUILTINS` and `DEFAULT`) |
| No camera-JPEG flash in the loupe while a raw renders; lensfun distortion zoomed to fill the frame (no empty corners) | `fix/raw-view-colors` | `crates/ui-egui/src/headless/tests/loupe_stand_in.rs`, tests in `crates/lensdb/src/tests.rs` | `crates/ui-egui/src/panels/detail.rs` (the loupe keeps the previous photo up), `crates/engine/src/media/session.rs`, `crates/engine/src/lens_profiles.rs`, `crates/lensdb/src/lib.rs` (fill-frame scale) | `loupe_stand_in`, `the_loupe_says`, `lightcraft-lensdb`, `quick_jobs`, `lens_profiles` |
| Auto AI Denoise on open: switches AI Denoise on once for a raw at or above an ISO (Settings ▸ AI Denoise, off by default); on top of upstream PRs #2929 / #2932 (own model, Auto Sync, one batch row) | `feat/auto-ai-denoise` | `crates/engine/src/denoise/auto.rs`, `crates/engine/src/tests/denoise/auto.rs`, `crates/ui-egui/src/panels/denoise_auto.rs`, `crates/ui-egui/src/tests/denoise_auto.rs` | engine `cmd/denoise.rs` (`denoise.autoOpen`, `switch`), `denoise/settings.rs`, `denoise/state.rs`, `library.rs`, `cmd/prefs.rs`; UI `panels.rs`, `panels/denoise.rs`, `dialogs/settings/import.rs`. Until #2929 / #2932 reach official `main`, the branch carries them; the merge into `custom` took upstream's Auto Sync for the AI Denoise switch (it replaces the `fix/edit-all-selected` hook in `cmd/denoise.rs`) | `denoise` in engine, `lightcraft-catalog`, `denoise settings` in ui-egui |
| Show or hide top-bar items (Settings ▸ Top Bar, or right-click the bar); the Discord button is gone | `feat/topbar-items` | `crates/ui-egui/src/panels/topbar_items.rs` and `topbar_items/` (`editor.rs`, `tests.rs`) | `panels/topbar.rs`, `panels.rs` (module), `state/settings.rs` (field), `dialogs/settings.rs` (tab), `menus/run.rs` + `menus/ui_commands.rs`, `icons.rs` and `links.rs` (Discord removed), `docs/control-protocol.md` | `topbar_items` |
| Camera Matching / No Camera Matching in the Profile picker, for cameras without measured matrices too; thumbnails follow the choice, Auto Sync carries it, no false "edited" flag. Carries upstream PR #2222 (`profile.camera_colour`) and extends it | `feat/camera-matching-profile` | `crates/ui-egui/src/panels/camera_colour.rs`, `crates/ui-egui/src/tests/camera_colour.rs` | `crates/develop` (`settings.rs` `CameraColour`, `lib.rs`, `presets.rs`), `crates/catalog/src/model.rs`, `crates/pipeline/src/lib.rs`; engine `camera_preview/colour.rs`, `cmd/develop.rs`, `cmd/previews.rs`, `files/load.rs` + `fs.rs`, `media/cache.rs` + `source.rs` + `session.rs` (thumbnail options), `presets.rs`, `smart.rs`; UI `panels/profiles.rs`, `panels/edit.rs` + `edit/sliders.rs`, `panels.rs`; `docs/camera-preview-colour.md` | `camera_preview`, `camera_colour`, `thumbnail`, `presets`, `lightcraft-develop` |

Every feature also has an entry in `docs/whats-new.md` and a row in `docs/parity-checklist.md`; clashes there are
fixed by the update script (both sides kept).

The tests of every feature, after a merge:

```bash
cargo test -p lightcraft-codecs --lib animated_gif
cargo test -p lightcraft-filepanel
cargo test -p lightcraft-engine --lib -- animated_gif denoise prefs quick_jobs lens_profiles camera_preview media presets files auto_sync
cargo test -p lightcraft-lensdb
cargo test -p lightcraft-develop
cargo test -p lightcraft-catalog
cargo test -p lightcraft-ui-egui --lib -- animated_gif selection_order split_divider before_after command_palette edit_all photo_menu topbar_items titlebar loupe_stand_in the_loupe_says denoise settings profile camera_colour thumbnail
```

## Known failures on this Mac (not caused by the fork)

- `accessibility::every_interactive_node_has_a_name_in_every_view_and_tool`: fails on official `main` too (a "▸" the
  test fonts can't draw).
