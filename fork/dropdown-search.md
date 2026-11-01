# Type to search in dropdown lists

## What it does

Type to search in dropdown lists: typing in an open list narrows it (label starts, then word starts, then substrings; disabled items never match), ↑/↓/PageUp/PageDown, ↩ or a click picks, Esc clears then closes, "No match"; a slim Search field (a `TextField`: IME, paste) shows once you type, and a click in it keeps the list open; an open list owns the keyboard (shortcuts and the held Backslash stand down). The combo boxes of Settings, Import, Export, Print, Contact Sheet, Keywords, Info, Credits, soft-proof Profile, the smart-album rule editor (its field list searches the submenu fields too) and the preset pickers (all presets, only rows in view drawn); the menus of the borderless dropdown buttons: Profile (all groups at once, hover preview kept), White Balance, Aspect Ratio, Point Curve, Tool Overlay, Mask Overlay, Filter Presets and the filter bar pickers (rows that are not options wait while text is typed). Not searchable: Zoom and Click Zoom (bottom bar, `panels/bottombar.rs` left alone for `feat/bottombar-quick-filters`; a letter typed over them still runs no shortcut), Mask Effect (has its own search), the bottom bar's Sort and other menus, the Preset "…" menus, mask component menus.

## Branch

`feat/dropdown-search`

## Own files

`crates/ui-egui/src/widgets/dropdown_search.rs`, `dropdown_search/presets.rs`, `dropdown_search/tests.rs`, `panels/edit/profile_search.rs`, `tests/dropdown_search_menus.rs`

## Hooks in official files

`widgets.rs` (module line; `dropdown_within` marks an open menu), `shortcuts.rs` (`handle` stands down while a list is open), `panels/detail.rs` (held Backslash), `tests.rs` (module line), `panels/edit.rs` (WB, Profile, `mod profile_search`), `panels/edit/sliders.rs` (`profile_menu` takes the search), `panels/edit/curve.rs`, `panels/filterbar.rs`, `panels/masking.rs`, `panels/right/crop.rs`, and the combo-box call sites (`dialogs.rs`, `dialogs/export.rs`, `credits.rs`, `print_view.rs`, `rules_editor.rs`, `import/body.rs`, `settings/{general,import,display}.rs`, `panels/denoise.rs`, `right/{info,keywords}.rs`, `edit/histogram.rs`: `.show_ui(` → `dropdown_search::pick(` / `.search(`, `Popup::menu(` → `dropdown_search::menu(` / `menu_pick(`), `docs/whats-new.md`, `docs/parity-checklist.md`

## Tests

`dropdown_search`, `dropdown_search_menus`
