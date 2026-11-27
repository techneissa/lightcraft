# Feature naming

LightCraft uses **Perspective Correction**, **Guided Perspective**, and **Perspective Guides** for its own geometry controls. The section includes Off, Auto, Guided, Level, Vertical and Full modes, so its name does not imply every mode is automatic.

The previous Upright terminology is a naming-review candidate, not a verified finding about trademark registration. These descriptive names are chosen to distinguish LightCraft's own implementation.

## Compatibility boundary

The `geometry.upright`, `geometry.guides` and `tool.guidedUpright` commands, saved tool keys, Rust enum variants and serialized settings remain stable. XMP `crs:PerspectiveUpright` and `UprightTransform_*` mappings and Lightroom import messages still describe the external format accurately. No rendering, import or export logic changes.

All 17 shipped non-English catalogs contain the new labels. The normal CI, perspective-mode tests and a visual panel/tool check remain required before this draft is marked ready.

## Regression coverage

The standard-library-only `display_names` integration tests scan production Rust string literals for the retired display names and preserve explicit compatibility exceptions. They also test identifier, comment and raw-string handling. These focused checks do not replace application or visual testing.

## Recolor, the whole-photo colour replacement (issue #460)

The Edit panel section that replaces one colour with another across the photo is called **Recolor** (techneissa's decision, the Decision comment on #460). It is the name the Masking panel's Recolor rows already have (issue #83): one word for changing a colour while keeping its shading. "Replace Color" is another company's feature name and is not used anywhere a user sees. The commands, settings keys and code say `colorSwap` / `color_swaps` (ids are not user-visible text, naming rule 2).

Recolor is a plain descriptive word; no register search was run for it.
