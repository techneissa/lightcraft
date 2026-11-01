# Rejected photos look the same in every view

## What it does

Rejected photos: Compare and Survey never dim a photo flagged as rejected (they drew a dark veil over it: L* 49.6 → 28.7, median ΔE00 19 from the loupe, so a rejected A7 V portrait looked darker, flatter and greyer there than in the loupe and the filmstrip); the grid dims it only with Settings ▸ Interface ▸ Grid ▸ **Dim Rejected Photos** (on by default, as in Lightroom Classic; saved in `ui.json` as `settings.dimRejected`, check box `check:settings.dimRejected`, agents `ui.set {"settings": {"dimRejected": false}}`). The filmstrip never dims (as before). The red reject flag marks it everywhere.

## Branch

`fix/consistent-colour-across-views`

## Own files

`crates/ui-egui/src/headless/tests/view_colour.rs`

## Hooks in official files

`panels/compare.rs` (`photo_tile`: the veil's 3 lines become a comment), `panels/grid.rs` (`cell`: the veil asks the setting), `state/settings.rs` (`dim_rejected`), `dialogs/settings/display.rs` (`interface_tab`: the check box and hint), `headless/tests.rs` (module), `docs/parity-checklist.md` (LR-LIB-FLAG), `docs/whats-new.md`

## Tests

`view_colour` (the A7 V test needs `LIGHTCRAFT_MACRAW_ARW`)
