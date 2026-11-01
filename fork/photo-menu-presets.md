# Right-click menu presets and backup

## What it does

Right-click menu presets and backup: three presets in one compact dropdown beside the heading (switch at once, Save to Preset, Clear Preset, Undo), Export / Import icon buttons for a `.json` backup (`kind` `lightcraft-photo-menu`, `version` 1; a wrong, newer or broken file is refused, unknown items skipped), `photoMenu.savePreset` / `switchPreset` / `clearPreset` / `undoPreset` / `export` / `import`, and the two lists fill the page's height.

## Branch

`feat/photo-menu-presets`

## Own files

`crates/ui-egui/src/panels/photo_menu/presets.rs`, `presets_ui.rs`, `backup.rs`, `presets_tests.rs`

## Hooks in official files

`photo_menu.rs` (modules, the commands' arms, `file_command`), `photo_menu/editor.rs` (the row's call, `list_height`), `state/settings.rs` (`photo_menu_presets`), `menus/ui_commands.rs`, `icons.rs` (`TrayOut`, `TrayIn`), `photo_menu/tests.rs`, `docs/control-protocol.md`, `docs/parity-checklist.md`, `docs/whats-new.md`

## Tests

`photo_menu`
