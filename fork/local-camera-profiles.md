# Local camera profiles

## What it does

Local camera profiles: your own profiles appear in the Profile picker (a profile file in the camera-profiles folder is its own row for its camera, named from its `name` field or file name, and no longer changes Camera Matching); catalog format v10 (also declares the reserved `macos` colour).

## Branch

`feat/local-camera-profiles`

## Own files

`crates/engine/src/camera_preview/local.rs` and `local_tests.rs`, `crates/engine/src/cmd/local_profiles.rs`, `crates/engine/src/tests/local_profiles.rs` and `smart_local.rs`, `crates/ui-egui/src/local_profile_offer.rs`

## Hooks in official files

`crates/develop` (`settings.rs` `CameraColour::{Local, Macos}` + `Profile::camera_profile`, `presets.rs`), `crates/catalog` (`journal.rs` `VERSION` 10, `model.rs` `is_edited`), `crates/pipeline/src/lib.rs` (`SourceInfo::colour_note`), engine `camera_profiles.rs` (local files, bundled-only `get` / `cache_key`), `camera_preview.rs` + `camera_preview/colour.rs` (`with_starting_colour_for`), `files/load.rs`, `media/source.rs` (`LoadOptions::profile_key`) + `session.rs` (keys), `smart.rs` + `cmd/previews.rs` (header stamp), `cmd/develop.rs` (`develop.profile`), `cmd/query.rs` (`profiles.local`), `cmd/convert.rs` + `export/prepare.rs` (DNG colour), `lib.rs` + `library.rs` (`local_profile_asked`), `apps/lightcraft-cli` (`calibrate --name`); UI `panels/camera_colour.rs` (rows), `panels/profiles.rs`, `panels/edit.rs`, `panels/edit/sliders.rs`, `state/dialog.rs` + `dialogs.rs` + `dialogs/confirm.rs` (the one-time switch question), `app/frame.rs`, `dialogs/merge.rs`. **Catalog version rule:** the number 10 is this fork's; if upstream reaches 10 with another meaning, the merge conflicts on `VERSION` and the format table in `journal.rs`: give this feature the next free number before shipping that merge

## Tests

`lightcraft-catalog`, `lightcraft-develop`, `camera_profiles`, `camera_preview`, `local_profiles`, `smart_local`, `thumbnail_hash`, `presets`, `camera_colour` in ui-egui
