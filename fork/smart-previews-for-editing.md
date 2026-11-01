# Smart Previews for editing

## What it does

Smart Previews for editing: Settings ▸ Performance ▸ Use Smart Previews for viewing and editing (off by default, saved per user). With it on, the grid, loupe at fit and fill, Compare, Survey and every slider edit render from a photo's *current* smart preview even when the original is reachable (`SourceRef::Preferred`, checked on the render worker); the original is read for zoom to 100 % (the Full level), export, Edit In, Open as Layers, print, contact sheet, slideshow and GIF export, DNG / Original export, Photo Merge, AI Denoise, Find Dust Spots and photos with no current smart preview. A stale smart preview or one made for another colour (No Camera Matching, a local profile as its file is now, macOS Colour) counts as missing. The proxy keeps the Profile picker's flags (smart preview encoding 4). Info says "Editing from Smart Preview"; switching it on asks once to build the missing ones; new imports build theirs. A render from the proxy matches the original's (median ΔE00 0.3 to 0.5).

## Branch

`feat/smart-previews-for-editing`

## Own files

`crates/engine/src/media/smart_editing.rs` and `smart_editing/tests.rs`, `crates/engine/src/cmd/smart_editing.rs`, `crates/ui-egui/src/smart_editing.rs`, `crates/ui-egui/src/tests/smart_editing.rs`

## Hooks in official files

`smart.rs` (`ENCODE_VERSION` 4, `SmartFacts` flags, `load_for_editing`, `current_for`), engine `media.rs` (module, `smart_edit` field), `media/cache.rs` (`source_ref_with` 2 calls), `media/source.rs` (`SourceRef::Preferred`, `DecodedSource::edit_proxy`, `serves`), `media/session.rs` (`export_job` → `original_job`, `thumb_job` → `reduced_mut`), `cmd.rs`, `cmd/previews.rs` (`original_only`, three items made `pub(crate)`), `cmd/masks.rs` (dust and blemish find read the original), `import/commit.rs` (`after_import`), `apps/lightcraft-web/src/wire.rs` (one arm); UI `lib.rs` + `app.rs` (`smart_editing_applied`), `app/prefs.rs` (`sync`), `state/settings.rs` (two fields), `state/dialog.rs` + `dialogs.rs` + `dialogs/confirm.rs` (`Dialog::SmartPreviewsForEditing`), `dialogs/settings/performance.rs` (the row), `panels/right/info.rs` (`smart_line`) + `panels/right.rs`, `docs/parity-checklist.md` (LR-CLOUD-SMARTPREVIEW), `docs/whats-new.md`. **Encoding rule:** `ENCODE_VERSION` 4 is this fork's; if upstream reaches 4 with another meaning, the merge conflicts on `smart.rs`: give this feature the next free number

## Tests

`smart_editing` in engine and ui-egui, `smart`, `media::`, `local_profiles`, `smart_local`
