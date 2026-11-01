# Arrange the Edit panel

## What it does

Right-click a section header for Move Up / Move Down / Hide This Section, the six sections as a checked list, Show All Sections and Reset to Default; saved with the UI state (old state loads unchanged); a hidden section keeps its adjustments and ⌘1 to ⌘5 bring it back; `view.editSections {order?, hidden?}`, `view.editSectionShow`, `view.editSectionMove`, `view.editSectionsReset`.

## Branch

`claude/workflow-qol`

## Own files

`crates/ui-egui/src/panels/edit_sections.rs` and `edit_sections/tests.rs`

## Hooks in official files

`panels.rs` (module), `state.rs` (`edit_sections` field), `panels/edit.rs` (the six `section(…)` calls became closures, drawn in the saved order), `panels/edit/sliders.rs` (one menu call), `menus/run.rs` (routing and the `section.*` reveal), `menus/ui_commands.rs`

## Tests

`edit_sections`
