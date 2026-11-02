# Hover help on every control, and a Solo Mode button

List: every icon-only button and toggle (and each text button whose label alone is unclear) shows short hover text, a test fails when a clickable icon has none, and each Develop section header has a Solo Mode button beside the eye; `fix/52-solo-hover-help`.

## What it does

- **Hover text.** 86 controls that showed nothing now say what they do (the audit's 69, 13 crop-overlay and Upright segments, the Print profile menu, the two Smart Album menus and the new Solo Mode button), and two wrong texts are fixed. They (Title Case, at most six words, the shortcut after it where the control has one): the bottom bar's flags, stars, Copy Settings gear, zoom menus and thumbnail slider; the sidebar and keyword triangles ("Collapse Albums"); each Develop section header ("Collapse Light") and its eye ("Turn Section Off" / "Turn Section On"); Auto, B&W, the profile, white balance, curve preset and proof colour menus; the Colour Mixer's band dots; the masking panel's eye, overlay menu and colours, Add / Erase, part buttons and effect menu; Crop, Straighten, Upright and Remove buttons, the vignette, overlay and Upright groups; the Remove "found" list; the Book's add-page tile and Auto Layout; the Map's filter buttons and Saved Locations; the Export dialog's All / None, the Keyboard Shortcuts slots, the lens folder Clear, the Print profile, the Smart Album field and operator menus, and the filmstrip navigator. The tool strip's "More" now says "About LightCraft" (it opens the About window) and the top bar's help icon says "Keyboard Shortcuts" (Title Case).
- **Shared helpers do the rest.** `widgets::section_header_solo` (header, eye, Solo button), `Rating::show` ("Rate 3 Stars"; the filter bar's stars say "Filter by 3 Stars"), `tree_toggle` and the keyword triangle (`disclosure_name`), `widgets::segmented_tips` (a tip per segment), `favorite_button` ("Add to Favorites" / "Remove from Favorites"). A dropdown takes `.on_hover_text(...)` at its call site.
- **Texts go through `i18n::tr`.** The English text is the key; a language without a translation shows English. Existing keys are reused where they exist ("Collapse {name}", "Copy Settings…").
- **Solo Mode button.** A small icon (`Icon::Solo`, one open panel between two folded ones) beside the eye at the right of each Develop section header. One click turns Solo Mode on or off (`view.soloSections`); it is lit on every header while the mode is on (the mode is global), and shows on a header while the pointer is on it. Hover text: "Turn Solo Mode On" / "Turn Solo Mode Off". The header's right-click entry stays.
- **The test.** `tests/hover_help.rs` drives the headless app through the main views, panels and dialogs with the accessibility tree on and fails for every clickable icon-only node (its name is not painted text) that has no description (`on_hover_text` sets the description). It lists the controls it cannot judge (the dock handles are craft-ui's, the top bar's background, the histogram, photo cells) in `exempt`. A second test proves the scan reports a bare icon. Add a screen to `scenes()` or a dialog command to `DIALOGS` when a new area gets controls.
- **Pointer over a child.** egui keeps the first hover text of a layer open while the pointer stays inside its widget, so a header does not ask for its own hover text while the pointer is on the eye or Solo button (and a sidebar header not over its own button); theirs shows instead.

## Branch

`fix/52-solo-hover-help`

## Own files

`crates/ui-egui/src/tests/hover_help.rs`

## Hooks in official files

- `widgets.rs`: `section_header_solo` and `SectionHeader` (`section_header` keeps its signature and calls it), `segmented_tips`; `widgets/rating.rs` (`Rating::filters`, the star tips, the favourite tip); `icons.rs` (`Icon::Solo`, drawn in code, original work, covered by the `icons.rs` row of `assets/ATTRIBUTION.md`); `headless.rs` (a test-only `a11y` field: the last frame's accessibility nodes); `tests.rs` (`mod hover_help;`).
- `panels/edit/sliders.rs` (the header call and the Solo click), `panels/edit.rs`, `panels/edit/{curve,histogram,mixer}.rs`, `panels/bottombar.rs`, `panels/filterbar.rs`, `panels/left/{rows,sections}.rs`, `panels/keyword_list.rs`, `panels/{masking,mask_parts,mask_effects,map,map_locations,strip,topbar}.rs`, `panels/right/{crop,remove_finds}.rs`, `panels/book/{settings,strip}.rs`, `panels/detail/filmstrip.rs`, `dialogs/{export,print_view,rules_editor}.rs`, `dialogs/settings/lens_database.rs`, `keybinds/window.rs`: one `.on_hover_text(tr("..."))` per control, or a `tip` argument.

## Tests

`cargo test -p lightcraft-ui-egui --lib -- hover_help`

## What's New entry

### Hover Help and a Solo Mode Button
- Controls that showed nothing on hover now say what they do: the eye on each Develop section, the stars, flags, zoom menus, the masking buttons, the Crop and Remove buttons and more.
- Each Develop section header has a Solo Mode button beside the eye: one click turns Solo Mode on or off, and it is lit on every header while it is on.
