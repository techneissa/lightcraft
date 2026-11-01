# Show in Finder after export

## What it does

An Export dialog option (File Explorer on Windows, the file manager on Linux) that shows the first exported file once the export succeeds; saved with presets and Export with Previous; the CLI, MCP and headless runs ignore it; a failing opener adds a note to the export message.

## Branch

`claude/workflow-qol`

## Own files

`crates/ui-egui/src/export_reveal.rs`, `crates/ui-egui/src/tests/export_reveal.rs`

## Hooks in official files

engine `export.rs` and `export/params.rs` (`show_in_finder` / `showInFinder`), `mcp/src/tools.rs` (and `recursion_limit = 256` in `mcp/src/lib.rs`), `dialogs.rs` (one row call), `export_task.rs`, `control.rs`, `lib.rs`, `tests/dialog_fit.rs` (the Export dialog is 30 pt taller)

## Tests

`export_reveal`, `export::tests`, `dialog_fit`
