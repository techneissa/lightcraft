# Auto AI Denoise on open

## What it does

Auto AI Denoise on open: switches AI Denoise on once for a raw at or above an ISO (Settings ▸ AI Denoise, off by default).

## Branch

`feat/auto-ai-denoise`

## Own files

`crates/engine/src/denoise/auto.rs`, `crates/engine/src/tests/denoise/auto.rs`, `crates/ui-egui/src/panels/denoise_auto.rs`, `crates/ui-egui/src/tests/denoise_auto.rs`

## Hooks in official files

engine `cmd/denoise.rs` (`denoise.autoOpen`, `switch`), `denoise/settings.rs`, `denoise/state.rs`, `library.rs`, `cmd/prefs.rs`; UI `panels.rs`, `panels/denoise.rs`, `dialogs/settings/import.rs`. Until #2929 / #2932 reach official `main`, the branch carries them; the merge into `custom` took upstream's Auto Sync for the AI Denoise switch (it replaces the `fix/edit-all-selected` hook in `cmd/denoise.rs`)

## Tests

`denoise` in engine, `lightcraft-catalog`, `denoise settings` in ui-egui

## When upstream merges

On top of upstream PRs #2929 / #2932 (own model, Auto Sync, one batch row).
