# Camera Matching in the Profile picker

## What it does

Camera Matching / No Camera Matching in the Profile picker, for cameras without measured matrices too; thumbnails follow the choice, Auto Sync carries it, no false "edited" flag.

## Branch

`feat/camera-matching-profile`

## Own files

`crates/ui-egui/src/panels/camera_colour.rs`, `crates/ui-egui/src/tests/camera_colour.rs`

## Hooks in official files

`crates/develop` (`settings.rs` `CameraColour`, `lib.rs`, `presets.rs`), `crates/catalog/src/model.rs`, `crates/pipeline/src/lib.rs`; engine `camera_preview/colour.rs`, `cmd/develop.rs`, `cmd/previews.rs`, `files/load.rs` + `fs.rs`, `media/cache.rs` + `source.rs` + `session.rs` (thumbnail options), `presets.rs`, `smart.rs`; UI `panels/profiles.rs`, `panels/edit.rs` + `edit/sliders.rs`, `panels.rs`; `docs/camera-preview-colour.md`

## Tests

`camera_preview`, `camera_colour`, `thumbnail`, `presets`, `lightcraft-develop`

## When upstream merges

Carries upstream PR #2222 (`profile.camera_colour`) and extends it.
