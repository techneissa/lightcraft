# Feature naming

LightCraft uses **Perspective Correction**, **Guided Perspective**, and **Perspective Guides** for its own geometry controls. The section includes Off, Auto, Guided, Level, Vertical and Full modes, so its name does not imply every mode is automatic.

The previous Upright terminology is a naming-review candidate, not a verified finding about trademark registration. These descriptive names are chosen to distinguish LightCraft's own implementation.

## Compatibility boundary

The `geometry.upright`, `geometry.guides` and `tool.guidedUpright` commands, saved tool keys, Rust enum variants and serialized settings remain stable. XMP `crs:PerspectiveUpright` and `UprightTransform_*` mappings and Lightroom import messages still describe the external format accurately. No rendering, import or export logic changes.

All 17 shipped non-English catalogs contain the new labels. The normal CI, perspective-mode tests and a visual panel/tool check remain required before this draft is marked ready.

## Regression coverage

The standard-library-only `display_names` integration tests scan production Rust string literals for the retired display names and preserve explicit compatibility exceptions. They also test identifier, comment and raw-string handling. These focused checks do not replace application or visual testing.

## Color Swap (issue #460)

The Edit panel section that replaces one colour with another across the photo is called **Color Swap**. Its first working name, "Replace Color", is another company's feature name and is not used anywhere a user sees; the commands, settings keys and code say `colorSwap` / `color_swaps`.

Checked on 2026-11-27 against `standards/naming.md` rule 4, with a web search only (the register sites need a browser form, which an agent run could not use):
- Descriptive use by unrelated products: Pixelmator Pro and Photomator call their feature "Replace Color"; Luminar describes a colour swap through masking; "color swap" and "swap colors" are plain words several free browser tools use for the same job; ColorSwapp is a phone app that does it.
- A US register search by an aggregator (Justia) found no "COLOR SWAP" mark, only unrelated COLOR SWITCH (games), COLOR SPLASH (a photo app mark) and COLORSNAP marks.
- Not done: the owner's trademark lists, USPTO, EUIPO and the WIPO Global Brand Database searched by hand. **The name is therefore not cleared.** Rule 5 says unclear means no: if a Craft owner prefers another name, the rename changes the labels only (`Color Swap` in `edit_sections.rs`, `controls.rs`, `presets.rs`, `edit.rs`, the panel and the docs), and the old label stays as an undisplayed search alias.
