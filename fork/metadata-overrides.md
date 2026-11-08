# Edit camera, lens and exposure values in Info

## What it does

Edit camera, lens and exposure values in Info (Camera, Lens, Focal length, Shutter speed, Aperture, ISO) for one or many photos: the file's values stay, edits are catalog overrides (`Meta.exif`, catalog format v14) shown with a small dot and a code-drawn ↺ reset button on every edited row (centred on the row's value, also when it wraps; no Reset All button or Edited label in the card; `photo.resetExif` stays), Mixed for a differing selection, searchable Camera / Lens pickers over lensfun's names (names only) and the Adobe-list camera names, validated number fields; search, filters, smart albums, rename tokens and export use the edited values, anything that renders keeps the file's camera and lens; `photo.setMeta` takes the six keys, `photo.resetExif`.

The card at rest looks like the official one: dim Camera, Lens, Focal length, Shutter speed, Aperture and ISO labels, plain values, no boxes (issue #28). A click on a value opens a text field for it alone (Return or a click away saves, Esc cancels); Camera and Lens open their searchable pickers the same way. The six rows are one height with a gap; a value too long for the column wraps (up to four lines) and moves only the rows below; editing moves nothing (the field takes the value's place, the picker list is an overlay). The file format badge stands at the card's top right level with Camera, the picture's size level with Aperture and the file size level with ISO.

An edited focal length or aperture has to fit the photo's lens, read from its name (`crates/catalog/src/lens_limits.rs`: "16-35mm f/4", "50mm f/1.8", "18-55mm F3.5-5.6" limits to f/3.5, "70-200mm F2.8 GM"; a name it cannot read sets no limit). `photo.setMeta` refuses a value outside it ("This lens opens to f/4 at most", "This lens covers 16–35 mm"), so agents get the rule too; the card checks first. A refused edit keeps the old value, the row shakes in a red outline for 0.3 s, and a small red "!" at the row's end opens a popup with the reason. A refused edit's "!" lasts until the next edit begins (a click on any value), a reset, or a change of photos or lens. A lens change keeps the edits and the "!" marks one now out of range; the file's own values are never checked. A teleconverter in the lens name ("+ 1.4x", "TC-20E") scales the limits; one whose factor cannot be read sets no limit; a decimal comma ("f/1,4") reads as a point. A call on many photos is all or nothing, and its error names the first refused photo and how many were refused. Clicking Camera or Lens opens their list of names at once.

## Branch

`claude/metadata-overrides`

## Own files

`crates/catalog/src/exif_override.rs` and `exif_override/tests.rs`, `crates/engine/src/cmd/exif_edit.rs`, `crates/engine/src/tests/exif_edit.rs`, `crates/lensdb/src/names.rs` and `data/camera-names.tsv`, `data/lens-names.tsv`, `tools/camera_lens_names.py`, `crates/catalog/src/lens_limits.rs` and `lens_limits/tests.rs`, `crates/ui-egui/src/panels/right/exif_edit.rs` and `exif_edit/tests.rs`, `crates/ui-egui/src/tests/exif_edit.rs`, and in sidecars (`feat/12-exif-xmp-sidecar`) `crates/engine/src/sidecar/exif.rs` and `exif/tests.rs`, `crates/engine/src/tests/exif_xmp.rs`

## Hooks in official files

catalog `model.rs` (`Meta.exif`), `lib.rs` (modules, re-export), `query.rs` + `rules.rs` (effective values), `journal.rs` (`VERSION` 14); engine `cmd.rs` (module, specs), `cmd/library/specs.rs` (`photo.setMeta` calls `exif_edit::apply_params`, which checks the lens limits and reports the refused photos), `cmd/query.rs`, `export/encoding.rs`, `rename/tokens.rs`, `slide_overlay.rs`, `files/probe.rs`, `tests.rs`; lensdb `lib.rs` (`pub mod names`); UI `panels/right.rs` (module), `panels/right/info.rs` (`camera_card`, registers `info:card`), `icons.rs` (`Icon::Reset`, `Icon::Alert`), `tests.rs`, `caches.rs` (filter lists), `panels/detail/info.rs`, `panels/grid.rs` and `panels/edit/histogram.rs` (shown values); `assets/ATTRIBUTION.md`, `NOTICE` (lensfun names, CC-BY-SA 3.0), `docs/parity-checklist.md` (LR-META-EXIF-EDIT), `docs/whats-new.md`; sidecars: the edits are written to and read from XMP sidecars (only overridden fields, `lc:exifOverrides` names them so a reset removes its tags; a differing sidecar value is read as an override) through engine `sidecar.rs` (`mod exif`, `SidecarData::exif`, `parse_sidecar`, `merge_into`, `sidecar_packet_with`, `sidecar_contents` owns the edited fields' tags) and meta `xmp.rs` (two `push` lines: `aux:Lens`, `exifEX:PhotographicSensitivity`), `docs/xmp-interop.md`. **Catalog version rule:** 14 is this fork's (13 is `feat/local-camera-profiles`; renumbered on 2026-11-08, upstream's 12 is shared segmentation grids); if upstream reaches 13 or 14 with another meaning, the merge conflicts on `VERSION` and the format table in `journal.rs`: give this feature the next free number

## Tests

`exif_override`, `lens_limits`, `exif_edit`, `exif_xmp`, `sidecar::exif`, `names::`

Picking (or typing and confirming) a prime lens in Info, a name with exactly one focal length before "mm" (`Sirui Aurora 35mm f/1.4`, `Laowa 60mm f/2.8 2X Ultra-Macro` = 60), also sets the Focal length as an edited override in the same undo step; zooms, ranges and names without a focal length leave it alone (`prime_focal_mm` in `exif_edit.rs`).
