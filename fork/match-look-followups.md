# Match Look follow-ups

List: Match Look keeps every slider move within a natural limit, and can add a residual Look (a colour lookup after the sliders, with its own Amount) for what sliders cannot reach. Branch `feat/90-match-look-followups`.

## What it does

**Limits.** Match Look (`fork/match-look.md`) moves each slider at most this far from where the photo had it, so a match looks natural at Amount 100: Exposure 1.5, Contrast, Whites, Blacks and Vibrance 40, Highlights and Shadows 50, Saturation 35, Tint 40, Temp 80 mired, each Color Grading wheel 30. A Color Mixer band moves at most hue 10, saturation 15, luminance 10, or hue 20, saturation 35, luminance 25 when its hue is at least 8 % of both photos: a colour the two share by chance (a blue T-shirt against a navy sofa) no longer drags a band to its end. Each slider also stays within a band whatever the move: Exposure ±3, Contrast, Whites, Blacks and Vibrance ±60, Highlights and Shadows ±70, Saturation ±50, Tint ±60, Temp 2500–12000 K, a mixer hue ±25, saturation ±50, luminance ±40, a grading wheel's saturation 40. A photo already past a band is not pulled in, only kept from going further. A bound that isn't a number (a damaged file) leaves that slider alone. The pull back to the photo's own sliders is four times stronger than before, so the fit prefers small moves. On a private test pair, Blue Luminance went from −90 to −20.

**Residual Look.** The Match Look window has a **Residual Look** box, off by default. On, the match also fits a colour lookup in OkLab after the sliders: a smooth lightness map, the cast of shadows, midtones and highlights, colourfulness, and a colour offset for each hue that is a large part of both photos. Each part is measured against the reference, refined over a few renders through the real pipeline, kept within small limits (lightness 0.04, cast 0.03, colourfulness 15 %, a hue 0.03 / 0.02 in OkLab), and kept only when the photo comes closer to the reference. The lightness map's ends stay at 0 and neighbouring knots differ by at most 0.02. The Look has 48 renders of its own, so sliders that use theirs up never leave it out. A photo matched to itself gets no Look. With the box ticked, a match replaces the photo's earlier Look; unticked, it keeps it (the sliders are fitted with it in place).

The Look is saved with the edit (`look` in the develop settings: `amount` 0..100 and the offsets). Left out of the JSON while empty, so photos without one keep their settings and cached previews. While a photo has one, the Edit panel's Profile area shows **Look Amount** (`look.amount`, a develop control like any slider). Match Look's Amount moves the Look from the photo's earlier one (none: Amount 0) to the matched one, so 1 % stays close to the earlier Look. B&W photos skip it.

The Look belongs to the Profile group: Copy / Paste / Sync Settings and presets of the Profile group carry it (a photo without a Look carries none, so pasting it removes one), a Profile reset clears it, and the Profile eye (`develop.sectionEnabled {section: "profile"}`; the panel draws no eye there) switches it off. That eye leaves a LUT profile as it was.

The Look is a field older builds don't know: the library format is fork 3 (`fork/catalog-format.md`), so a fork 2 build refuses a library written by this one instead of opening it and dropping every Look when it saves.

The render bakes the Look into a 17³ lookup table on the display-encoded colour and puts it in the finishing stage's LUT slot, where a `.cube` profile goes (composed with one, 33³, when the photo has both: about 5 ms per change of Look Amount on the Mac mini). The last table is cached, keyed on the Look, the profile's id and the LUT registry's generation (bumped when a LUT is registered or forgotten), and built outside the cache's lock. The CPU loop and the GPU kernel read the same slot, so they match, and the view and an export render it alike.

Commands: `develop.matchLook` takes `options.look` and returns `look` per photo; `matchLook.apply {look?}` sets the window's box.

## Branch

`feat/90-match-look-followups` (from `custom`, after `feat/84-match-look`)

## Own files

`crates/develop/src/residual_look.rs`, `crates/pipeline/src/residual_look.rs`, `crates/look/src/residual.rs`

## Hooks in official files

`crates/develop`: `lib.rs` (module and re-export, a Profile reset clears the Look), `settings.rs` (the `look` field and its start value), `controls.rs` (`look.amount`), `presets.rs` (`look` in the Profile group); `crates/pipeline`: `lib.rs` (module), `finish.rs` (the LUT slot goes through `residual_look::finish_lut`), `lut.rs` (the registry's generation); `crates/catalog`: `fork_format.rs` (`VERSION` 3), `journal.rs` (the fork 3 row), `tests_format_version.rs`, `tests_fork_format.rs`, `tests_journal_delta.rs`; `crates/ui-egui/src/panels/edit.rs` (the Look Amount slider). Match Look's own files: `crates/look` (`fit.rs` caps and mixer bands, `params.rs` caps and prior, `stats.rs` shares the hue band and zone helpers with the pipeline, `lib.rs` option and result, `examples/match.rs --look`), `crates/engine/src/cmd/match_look.rs`, `crates/ui-egui/src/panels/match_look.rs`, and their tests.

## Tests

Commands in `fork/tests.d/match-look-followups.txt`. A wild reference never moves a slider past its cap; a near-identical pair moves little; the Look is off by default, empty for the same look, and carries a cast the sliders leave (lower loss); a new match removes an earlier Look; the Look's table is empty without a Look (JSON and hash unchanged), warms greys with a warm cast, scales with its Amount, stays in range on hostile numbers, composes with a LUT profile, and shows in a real render; the engine saves the Look with `options.look`, `look.amount` scales it, undo removes it; the window's box is off by default and turns on; the Look Amount slider shows only with a Look; caps bound the final value (Saturation 40 stops at 50, 70 stays at 70); the Look runs on a budget the sliders used up; non-finite settings are no panic; 20,000 random lightness maps end at 0 and stay smooth; an unticked match keeps the Look, a small Amount stays near the earlier one; Copy / Paste and a preset carry it, a Profile reset clears it, the Profile eye hides it; a LUT profile changed at the same address rebuilds the composed table; a library with a Look is fork format 3.

## What's New entry

### Match Look: natural limits and a residual Look
- Match Look keeps each slider within a natural range, and moves a Color Mixer band far only when its colour is a large part of both photos.
- A new **Residual Look** box in the Match Look window adds a gentle colour lookup after the sliders for what they cannot reach, with its own **Look Amount** in the Edit panel.
