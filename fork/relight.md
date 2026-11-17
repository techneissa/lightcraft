# Relight

A Develop section with lights you place on the photo. Each light brightens what is near it in depth and faces it, live while you drag. Depth comes from the photo's own depth map or an on-device model.
History is in the PRs: #288 (the feature), #294 (⌥ overlay), #299 (several lights), #300 (types), #302 (gel, pattern, haze), #315 (shadows), #316 (patterns), #301 (presets), #317 (beams), #319 (Rim side), #293 and #322 (depth smoothing).

## What it does

- **Lights (#288):** the section sits between Lens Blur and Calibration. It has the list of lights, a depth picture, a Near–Far track, Amount, Softness, Warmth, and an Advanced flyout (Darken Surroundings, Wrap).
  Handles sit on the photo; one drag is one undo step. Light is added in scene-linear light, with a roll-off to a ceiling and deep shadows held back. Full Amount is three stops at the most lit point.
- **Several lights (#299):** up to 8, at least 1. Each has its own place, depth and colour. Darken Surroundings is one value for the section. Selection is UI state (`Session::relight_selected`), not saved.
  Each light's lobe (`Field::lobe`) is made once and kept (`pipeline::relight::LOBES`, `forget()` frees them). Lobes add up in linear light; colour planes follow each light's tilt. A scroll over a handle moves its depth.
- **Types (#300):** Point (the original formula, byte for byte), Spot (aim, Cone Angle, Cone Edge), Sun / Window (azimuth, elevation, no fall-off), Softbox (rectangle at a depth, aimed by `boxAimH`/`boxAimV` ±80°, #335), Rim (outline of depth steps, #319: `rimAngle`, `rimSide`), Fill (lifts shadows).
  A type only changes each pixel's `falloff · facing`, so everything after that is shared. `KindSettings::read` is lenient: an unknown kind reads as a Point.
- **Effects (#302):** a gel (hue and strength, five named gels) goes through the finish stage's own temperature and tint, so the CPU and GPU cannot differ. The colour planes are four: warm, cool, magenta, green. A pattern (blinds, panes, leaves, dapple) and Haze are per light.
- **Patterns (#316):** a projector throws the pattern into its window or gap and onto the first surface the light reaches. Edges blur with distance (`projector.rs`, `pattern.rs`, `leaves.rs`).
  The subject blocks it (a patterned light always casts full shadows). Leaves are baked once before the parallel loop. The shade passes a second, tinted lobe (`Effects::through_colour`). Openness sets the gap.
- **Shadows (#315):** a Cast Shadows slider per light. Rays march through the depth map on a reduced field (`Reduced`) and are blocked by "things": stuff with farther depth on both sides, as deep as it is wide (`occluder.rs`, `shadow.rs`).
  A `Visibility` is kept per light (`SHADOWS`) and upsampled into the lobe. A drag marches coarser; the release and export march in full. A Rim and a Fill take no shadow.
- **Beams (#317):** Haze shows the light's beams (`beam.rs`). Each line of sight sums single-scattered light, cut by the same blockers as shadows plus a slab for walls, with open sky letting light through.
  Forward Scatter (`scatter`) and Dust (`dust`) shape it. The beam is added on top in the light's colour as extra mask planes (`LocalAdjustments::fork_added`, never saved), not a gain. Shafts are kept until the light moves. A Rim and a Fill make no beam.
- **Mist and fog (#331):** Low Mist and Mist Height thicken the haze near a ground plane fitted from up-facing normals (`mist.rs`), not by image row.
  Distance Fade (`c·T + air·(1−T)`) is one per photo: the strongest light's numbers, coloured by the far tenth plus 30 % of the lights with fade on; sent as an exposure plane plus a `fork_added` plane.
- **Stage beams (#334):** Cone Angle goes down to 2°. Sharp Cone (`spotSharp`) makes the cone's edge a fixed ramp of a tenth of the angle (a hard pool on a wall; in haze a beam's flank is round whatever the edge). Visible Source (`source`, 0–100) draws a light inside the photo as a bright point (`source.rs`), hidden by nearer things, none for a Sun / Window, as a mask plane of its own (`Beamed::slot`) kept per place and strength, not in the shading key.
  Concert Backlight, Two-Colour Stage and Single Spotlight are built-in presets with soft cones and high Wrap: a hard edge on a face reads as a sticker, and the depth map's noise makes a near-hard pool jagged there.
- **Presets (#301):** a Light Presets flyout lists ten built-ins (Rembrandt, Butterfly, Split, Rim + Fill, Golden-Hour Window, Neon Two-Tone, Candle and the three stage ones, #334) and your own.
  Presets are written for a face and placed round the photo's subject (`Subject::place`, depth from `Field::depth_in`). Applying is one undo step. Yours live in the library's `presets.json` (`relightPresets`), read entry by entry.
  `relight.applyPreset` returns `{applied: false, finding: true}` while depth is still found.
- **⌥ view (#294):** holding ⌥ while dragging a Relight control shows where the light reaches (Crosses or Contours, `UiState::relight_look`, synced as `relightLook`). It is drawn on a worker from the render's shading (`lightcraft_relight::overlay`) and never changes the render.
- **Depth:** found off the UI thread by one worker, shown in the top-bar ring, cancellable. Fields are kept in memory (300 MB, most recently used first), never on disk.
  The model is Depth Anything 3 Mono Large, in pure Rust with candle, downloaded into the models folder and never in the repo. The web build has no Relight.
- **Field (`field.rs`, #293, #322):** the depth is upsampled to a 1600 px guide by a joint bilateral filter, smoothed against the photo's brightness, and kept within its own side of a step. Normals come from that depth.
- **Where it travels:** like masks and spots, it stays on its photo. Keep in Sync and Portrait presets leave it out; Copy Settings offers the group unticked.
- **Commands:** `relight.status`, `findDepth`, `downloadModel`, `addLight`, `duplicateLight`, `deleteLight`, `selectLight`, `setGel`, `applyPreset`, `savePreset`, `renamePreset`, `deletePreset`, `presets`. Controls: `relight.<n>.<name>` per light and `relight.<name>` for the selected light.
- **Tuning outside the app:** `cargo run --release -p lightcraft-depth --example depth -- <model dir> photo.jpg depth.png --dump dump/`, then `cargo run --release -p lightcraft-relight --example relight -- photo.jpg dump/ out [x y depth amount softness warmth darken wrap]`.

## Catalog formats

`fork_format.rs` and `journal.rs` rows, written only when a value differs from its default. An older build refuses a newer library.

- 11: one light and `darken` in one object (`fork_relight`).
- 12: `{lights: [...], darken}`, up to 8. A format 11 log patch is moved to the first light on replay (`Replay::first_form_relight`).
- 13: gel, pattern and haze keys.
- 14: `kind` and each type's numbers.
- 15: `rimAngle`, `rimSide`.
- 16: `shadows`.
- 17: `patternOpen`.
- 18: `scatter`, `dust`.
- 19: `mist`, `mistHeight`, `fade` (#331).
- 20: `boxAimH`, `boxAimV` (#335). An aimed patterned softbox's far source sits along −n from its pool's middle, so shaft and shadows follow the aim.
- 21: `spotSharp`, `source` (#334).
- If the upgrade snapshot cannot be written, the journal appends nothing (`Journal::upgrade_pending`) and retries before the next append.

## Own files

- Crates: `crates/depth/`, `crates/relight/` (`light`, `field`, `overlay`, `effects`, `shadow`, `occluder`, `kinds`, `rim`, `projector`, `pattern`, `leaves`, `probe`, `beam`, `tests_*`, `examples/relight.rs`).
- develop: `relight.rs`, `relight_effects.rs`, `relight_kinds.rs`, `relight_presets.rs`.
- pipeline: `relight.rs`, `tests_relight.rs`, `tests_relight_presets.rs`.
- gpu: `tests/it/relight.rs`.
- engine: `relight.rs`, `relight_presets.rs`, `relight_presets/tests.rs`,
- engine: (more) `cmd/relight.rs`, `cmd/relight_presets.rs`.
- ui-egui: `panels/edit/relight.rs`, `panels/edit/relight/{effects,kinds,presets}.rs`, `panels/detail/relight.rs`, `panels/detail/relight/kinds.rs`, `panels/detail/relight_alt.rs`, `state/relight_look.rs`.
- Docs: `fork/relight.md`, `fork/tests.d/relight.txt`.

## Hooks in official files

- Build: `Cargo.toml` (the two workspace dependencies), `xtask/src/layers.rs` (depth L3, relight L1), `NOTICE` (the depth model), and each crate's `Cargo.toml` below.
- develop: `lib.rs` (`pub mod` for the four modules, `reset_section` resets all lights), `settings.rs` (`fork_relight` and its default, `LocalAdjustments::fork_added`), `controls.rs` (`Section::Relight` and the specs), `presets.rs` (`SettingsGroup::Relight`, `default_copy` leaves it unticked).
- catalog: `fork_format.rs`, `journal.rs` (a row per format), `tests_format_version.rs`, `tests_fork_format.rs`, `fork/catalog-format.md`.
- pipeline: `lib.rs` (`pub mod relight;`, test mods, `SourceInfo::relight`, `Plan::relight`, the light's planes added to `prep.masks`), `retouch_step.rs` (`relight: None`), `finish.rs` (three more mask sums and the add in `adjust_scene_pixel`).
- gpu: `masks.rs` (`upload_relit`), `wgsl/finish.wgsl` (the beam add), `decontam.rs` (its anchor), `tests/it/main.rs` (`mod relight;`).
- engine: `lib.rs` (`Session::relight`, `relight_selection`, `relight_presets`, Keep in Sync leaves `fork_relight` out), `cmd.rs`, `cmd/develop.rs` (shorthand controls), `cmd/query.rs` (`develop.controls`), `library.rs` (`PresetsFile`, `relightPresets`),
- engine: (more) `portrait_presets.rs`, `people_parsing.rs` (models folder), `media/session.rs` and `media/source.rs` (field and its key), `export/prepare.rs` (`RenderWork::relight`), `files/load.rs` and `smart.rs` (`relight: None`).
- ui-egui: `panels/edit.rs`, `edit_sections.rs` and `edit_sections/tests.rs` (`EditSection::Relight`), `menus/ui_commands.rs` (`section.relight`), `panels/detail.rs` (handles, `claim_scroll`, `relight_alt`),
- ui-egui: (more) `state.rs` (`relight_look`), `settings_sync/rules.rs` (`relightLook`), `headless.rs`, `Cargo.toml`.
- `fork/section-arrange.md` (eight sections) and `docs/parity-checklist.md` (the row cites `relight.setGel`).

## Known limits

- Shadows march through one picture's depth: nothing is known behind a thing's front, outside the picture, or in hair. Things under about two reduced pixels can be missed.
- A thing standing on a surface does not block where it touches it, so a contact shadow fades out.
- A blocker's own far side can be a little grainy for a big light.
- A spot's hard edge (Cone Edge 0) leaks a little onto steep, evenly toned walls (the edge refit).
- A Rim follows the depth model's edges, so on a staircase it also lights step edges.
- Window panes pass only their open share, so a pane-cut beam is fainter (about 0.55 of a plain softbox's, a sun through panes 0.26 of a plain sun's); the air itself isn't dimmed.
- A drag of a sun in haze is a little softer than the release.
- A surface that is no blocker counts as a quarter of its depth deep to the air's rays, and open sky is far and bright, so a dark sky or a thick wall can let light through.
- The air has no shadow from beyond the picture's edges.

## Tests

`fork/tests.d/relight.txt` lists the modules:

- `lightcraft-relight` (`tests_edge`): no halo along a depth edge.
- `lightcraft-relight` (`light`): terraced floors, grazing normals, hostile depth and numbers, filters, Amount, Darken, Warmth.
- `lightcraft-relight` (`tests_kinds`, `tests_rim`): each type's shape, and a Point equals the old formula to the bit.
- `lightcraft-relight` (`effects`, `tests_pattern`): gel colours, pattern windows, blur with distance, blinds, the subject blocking.
- `lightcraft-relight` (`tests_shadow`): blockers, penumbra, thin poles, smooth change with depth, a drag's coarse march.
- `lightcraft-relight` (`tests_beam`): shafts, Forward Scatter, Dust, hidden behind a subject, a drag matches the release.
- `lightcraft-relight` (`overlay`, `probe`): contour lines; subject depth.
- `lightcraft-depth`: input sizes, broken output, f16 and f32, hostile headers.
- `lightcraft-develop` (`relight`, `relight_effects`, `relight_kinds`, `relight_presets`): save only non-defaults, lenient reads, ranges, the first form, subject placing.
- `lightcraft-catalog` (`fork_format`, `history_serde`): old log patches replay after each format upgrade; a failed upgrade snapshot leaves no mixed log.
- `lightcraft-pipeline` (`tests_relight`, `relight::tests`, `tests_relight_presets`): no change when off, colours, two lights, draft equals export, lobe memo, keys, shadows, beams, presets.
- `lightcraft-gpu` (`--test it -- relight`): the GPU equals the CPU for each light kind, effect, shadow and beam.
- `lightcraft-engine` (`--features sam`, `relight_presets`): light commands and undo, field store, failed and cancelled depth, where the light travels, preset placing and saving.
- `lightcraft-ui-egui` (`edit_sections`): the section order.

## What's New entry

### Relight
- A new Develop section, Relight, adds a light you place on the photo. Drag its handle and the light reaches what is near it in depth and faces it, live while you drag. Amount, Softness and Warmth shape it; Darken Surroundings and Wrap are under Advanced.
- Relight holds up to eight lights: Add Light, Duplicate and Delete under the list, click a row or a handle to pick one, drag the ring around the picked handle to change its size and scroll over a handle to move it nearer or farther. The lights add up, each with its own colour.
- Each light has a type (Point, Spot, Sun / Window, Softbox, Rim, Fill) and an Effects group: a gel (five named ones, or any hue and strength), a pattern (blinds, window panes, leaves or dapple) and haze.
- A pattern stays inside its window or the gap in the leaves. Its edges are sharp near the blinds or leaves and soft farther away, and the person in the photo blocks it.
- Lights cast shadows: a Cast Shadows slider (0 by default) makes the things in the photo block the light. A small light gives a hard shadow, a big one a soft one.
- Haze shows the light's beams: the air glows where the light reaches it, a nearer subject hides the beam behind it, and things in the photo cut it into shafts. Forward Scatter makes beams glow most when you look into the light; Dust makes the haze uneven.
- Light Presets: one click sets up several lights round the person in the photo. Seven built-in looks, each with a small picture of your photo. Save Lights as Preset… keeps your own.
- Hold ⌥ while dragging a Relight control to see where the light reaches: small crosses on the surfaces, or lines of equal light.
- Depth comes from the photo's own depth map (iPhone Portrait photos) or from an on-device depth model, downloaded once (about 1.3 GB). Saved with the edit; it stays on its photo unless you tick Relight in Copy Settings or a preset. The export matches the preview. The web version has no Relight.

## Parity rows

| LR-EDIT-RELIGHT | Relight: depth-aware lights (up to 8) dragged on the photo | P2 | 🟡 | `cmd:relight.status`, `cmd:relight.findDepth`, `cmd:relight.downloadModel`, `cmd:relight.addLight`, `cmd:relight.duplicateLight`, `cmd:relight.deleteLight`, `cmd:relight.selectLight`, `crates/relight/src/light.rs`, `crates/ui-egui/src/panels/edit/relight.rs` | beyond Lightroom (no equivalent): up to eight lights of six types (each with its own colour, gel, pattern, haze and shadows) placed in the photo's depth (its own map or Depth Anything 3), in scene-linear light; 🟡 until tried by hand |
