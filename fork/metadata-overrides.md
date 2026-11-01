# Edit camera, lens and exposure values in Info

## What it does

Edit camera, lens and exposure values in Info (Camera, Lens, Focal length, Shutter speed, Aperture, ISO) for one or many photos: the file's values stay, edits are catalog overrides (`Meta.exif`, catalog format v11) shown with a dot and a code-drawn ↺ reset button on every row plus Reset All, Mixed for a differing selection, searchable Camera / Lens pickers over lensfun's names (names only) and the Adobe-list camera names, validated number fields; search, filters, smart albums, rename tokens and export use the edited values, anything that renders keeps the file's camera and lens; `photo.setMeta` takes the six keys, `photo.resetExif`.

## Branch

`claude/metadata-overrides`

## Own files

`crates/catalog/src/exif_override.rs` and `exif_override/tests.rs`, `crates/engine/src/cmd/exif_edit.rs`, `crates/engine/src/tests/exif_edit.rs`, `crates/lensdb/src/names.rs` and `data/camera-names.tsv`, `data/lens-names.tsv`, `tools/camera_lens_names.py`, `crates/ui-egui/src/panels/right/exif_edit.rs` and `exif_edit/tests.rs`, `crates/ui-egui/src/tests/exif_edit.rs`, and in sidecars (`feat/12-exif-xmp-sidecar`) `crates/engine/src/sidecar/exif.rs` and `exif/tests.rs`, `crates/engine/src/tests/exif_xmp.rs`

## Hooks in official files

catalog `model.rs` (`Meta.exif`), `lib.rs` (module, re-export), `query.rs` + `rules.rs` (effective values), `journal.rs` (`VERSION` 11); engine `cmd.rs` (module, specs), `cmd/library/specs.rs` (`photo.setMeta` calls `exif_edit::apply_params`), `cmd/query.rs`, `export/encoding.rs`, `rename/tokens.rs`, `slide_overlay.rs`, `files/probe.rs`, `tests.rs`; lensdb `lib.rs` (`pub mod names`); UI `panels/right.rs` (module), `panels/right/info.rs` (`camera_card`), `icons.rs` (`Icon::Reset`), `tests.rs`, `caches.rs` (filter lists), `panels/detail/info.rs`, `panels/grid.rs` and `panels/edit/histogram.rs` (shown values); `assets/ATTRIBUTION.md`, `NOTICE` (lensfun names, CC-BY-SA 3.0), `docs/parity-checklist.md` (LR-META-EXIF-EDIT), `docs/whats-new.md`; sidecars: the edits are written to and read from XMP sidecars (only overridden fields, `lc:exifOverrides` names them so a reset removes its tags; a differing sidecar value is read as an override) through engine `sidecar.rs` (`mod exif`, `SidecarData::exif`, `parse_sidecar`, `merge_into`, `sidecar_packet_with`, `sidecar_contents` owns the edited fields' tags) and meta `xmp.rs` (two `push` lines: `aux:Lens`, `exifEX:PhotographicSensitivity`), `docs/xmp-interop.md`. **Catalog version rule:** 11 is this fork's (10 is `feat/local-camera-profiles`); if upstream reaches 10 or 11 with another meaning, the merge conflicts on `VERSION` and the format table in `journal.rs`: give this feature the next free number

## Tests

`exif_override`, `exif_edit`, `exif_xmp`, `sidecar::exif`, `names::`
