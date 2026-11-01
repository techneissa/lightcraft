# Shorter Settings

## What it does

Shorter Settings: help lines removed under Delete Permanently, Only for picked or starred photos and Starting Colour for Raw Photos; the Auto AI Denoise line cut to one sentence; Settings ▸ Import ▸ AI Denoise from removed (the engine preference `import.denoiseAboveIso` stays for agents, so the upstream PR merges cleanly).

## Branch

`fix/shorter-settings-text` (from `custom`: the texts are fork-only)

## Own files

none

## Hooks in official files

`dialogs/settings/general.rs`, `dialogs/settings/import.rs`, `dialogs/settings/starting_colour.rs`, `panels/denoise_auto.rs`; docs `denoise.md`

## Tests

`cargo test -p lightcraft-ui-egui --lib settings`
