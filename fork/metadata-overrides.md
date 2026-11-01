# Edit camera, lens and exposure values in Info

## What it does

Edit camera, lens and exposure values in Info (Camera, Lens, Focal length, Shutter speed, Aperture, ISO) for one or many photos: the file's values stay, edits are catalog overrides (`Meta.exif`, catalog format v11) shown with a small dot and a code-drawn ↺ reset button on every row plus Reset All (only while something is edited), Mixed for a differing selection, searchable Camera / Lens pickers over lensfun's names (names only) and the Adobe-list camera names, validated number fields; search, filters, smart albums, rename tokens and export use the edited values, anything that renders keeps the file's camera and lens; `photo.setMeta` takes the six keys, `photo.resetExif`.

The card at rest looks like the official one: dim Camera, Lens, Focal length, Shutter speed, Aperture and ISO labels, plain values, no boxes (issue #28). A click on a value opens a text field for it alone (Return or a click away saves, Esc cancels); Camera and Lens open their searchable pickers the same way.

An edited focal length or aperture has to fit the photo's lens, read from its name (`crates/catalog/src/lens_limits.rs`: "16-35mm f/4", "50mm f/1.8", "18-55mm F3.5-5.6" limits to f/3.5, "70-200mm F2.8 GM"; a name it cannot read sets no limit). `photo.setMeta` refuses a value outside it ("This lens opens to f/4 at most", "This lens covers 16–35 mm"), so agents get the rule too; the card checks first. A refused edit keeps the old value, the row shakes in a red outline for 0.3 s, and a small red "!" at the row's end opens a popup with the reason. A lens change keeps the edits and the "!" marks one now out of range; the file's own values are never checked.

## Branch

`claude/metadata-overrides`

## Own files

`crates/catalog/src/exif_override.rs` and `exif_override/tests.rs`, `crates/engine/src/cmd/exif_edit.rs`, `crates/engine/src/tests/exif_edit.rs`, `crates/lensdb/src/names.rs` and `data/camera-names.tsv`, `data/lens-names.tsv`, `tools/camera_lens_names.py`, `crates/catalog/src/lens_limits.rs` and `lens_limits/tests.rs`, `crates/ui-egui/src/panels/right/exif_edit.rs` and `exif_edit/tests.rs`, `crates/ui-egui/src/tests/exif_edit.rs`, and in sidecars (`feat/12-exif-xmp-sidecar`) `crates/engine/src/sidecar/exif.rs` and `exif/tests.rs`, `crates/engine/src/tests/exif_xmp.rs`

## Hooks in official files

catalog `model.rs` (`Meta.exif`), `lib.rs` (modules, re-export), `query.rs` + `rules.rs` (effective values), `journal.rs` (`VERSION` 11); engine `cmd.rs` (module, specs), `cmd/library/specs.rs` (`photo.setMeta` calls `exif_edit::apply_params`, which checks the lens limits), `cmd/query.rs`, `export/encoding.rs`, `rename/tokens.rs`, `slide_overlay.rs`, `files/probe.rs`, `tests.rs`; lensdb `lib.rs` (`pub mod names`); UI `panels/right.rs` (module), `panels/right/info.rs` (`camera_card`, registers `info:card`), `icons.rs` (`Icon::Reset`, `Icon::Alert`), `tests.rs`, `caches.rs` (filter lists), `panels/detail/info.rs`, `panels/grid.rs` and `panels/edit/histogram.rs` (shown values); `assets/ATTRIBUTION.md`, `NOTICE` (lensfun names, CC-BY-SA 3.0), `docs/parity-checklist.md` (LR-META-EXIF-EDIT), `docs/whats-new.md`; sidecars: the edits are written to and read from XMP sidecars (only overridden fields, `lc:exifOverrides` names them so a reset removes its tags; a differing sidecar value is read as an override) through engine `sidecar.rs` (`mod exif`, `SidecarData::exif`, `parse_sidecar`, `merge_into`, `sidecar_packet_with`, `sidecar_contents` owns the edited fields' tags) and meta `xmp.rs` (two `push` lines: `aux:Lens`, `exifEX:PhotographicSensitivity`), `docs/xmp-interop.md`. **Catalog version rule:** 11 is this fork's (10 is `feat/local-camera-profiles`); if upstream reaches 10 or 11 with another meaning, the merge conflicts on `VERSION` and the format table in `journal.rs`: give this feature the next free number

## Tests

`exif_override`, `lens_limits`, `exif_edit`, `exif_xmp`, `sidecar::exif`, `names::`
