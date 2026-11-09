# Assisted Culling: eyes closed and eyes out of focus

List: Photo ▸ Assisted Culling also reads the faces: Eyes Closed and Eyes Out of Focus per photo (rule-editor fields, Reject suggestions: the eyes never reject by themselves), a burst's best frame has open, sharp eyes, and the run goes on worker threads under a stoppable activity row (issue #147). Branch `feat/ai-culling`.

## What it does

Assisted Culling (`photo.analyze`, Photo ▸ Assisted Culling…) scored focus, clipping and bursts. It now also reads
the eyes of each photo's faces, from the face mesh's 478 points:

- **Eyes Closed**: each eye's aspect ratio (the lids' distance at three points over the corners' distance, in 3-D
  so a slightly turned face reads the same). A face whose eyes average under 0.11 has them closed; under 0.08 when
  only one eye can be read. On private test portraits open eyes read 0.24–0.41, eyes looking down 0.13–0.22, blinks
  and eyes squeezed shut 0.02–0.10. An eye narrower than 6 px of the 2048 px picture, the far eye of a face turned
  more than about 3/4 (narrower than 0.6 of the near one), both eyes of a face in profile, and a face the mesh is not
  sure of (score under 0.5) are **unknown**, never open.
- **Eye focus** (open eyes only): the strongest detail (top 10 % of a smoothed Laplacian's energy) in a box placed by
  the mesh round each eye, against the 75th percentile of eye-sized blocks of the upper face (down to the nose tip: a
  beard has more fine detail than any eye) and half the 90th percentile of the photo's blocks, measured at the scale
  where the eye is about 32 px wide. 100: the sharpest eye has the detail of the sharpest parts of the face and photo.
  Below 35 the eyes are **out of focus**, so a shot focused on an ear or on the background is caught even when the
  photo's own focus score is high.
- **Main faces**: the widest face and every face at least 0.6 as wide. Bystanders do not count. A photo's Eyes Closed
  is any main face with its eyes closed, and its Eyes Out of Focus is any main face (with open eyes) out of focus:
  the photo's eye focus is the weakest of its main faces. A main face that could not be read leaves the photo
  **unknown** (not open, not in focus) unless another main face already settles it (closed, or out of focus).
  A near-flat frame (a blank wall: almost no detail to compare the eyes with) has an unknown focus. A face one of
  whose eyes cannot be read (a missing landmark) is read from the other eye, with the one-eye line.
- **Stored** with the other culling scores (`Analysis.eyes`: faces, main faces, closed, focus): fork catalog format 9
  (7 and 8 are held for Background and the Refine brush). Rule-editor / smart-album fields `eyesClosed` and
  `eyesOutOfFocus` (Assisted Culling group); unknown eyes match neither yes nor no.
- **Bursts**: a burst's best is the sharpest frame whose eyes are open and in focus (a closed-eyes frame loses 100
  focus points, a soft-eyes frame 30, a frame whose eyes could not be read 15: it ranks below an open, sharp frame a
  few points less sharp); a burst that blinks throughout keeps its sharpest.
- **Suggestions for the eyes, official behaviour for focus**: the existing "Reject below focus" slider still rejects
  during the run, as before: photos under the score are flagged rejected when the run ends. Only the new eye results
  wait for a tick: `photo.analyze` lists `suggestReject` (`eyesClosed`, `eyesOutOfFocus`), and in the app a Reject
  Photos? dialog follows the run with a box per kind (each ticked while it has photos); only what is ticked is
  rejected, in one undo step (Reject Suggested Photos). A photo the slider rejected is not suggested again.
- Eyes measured before are kept (a run with `eyes: false` keeps them too); `eyes: "again"` measures them again, and
  keeps the old ones when the models are not there to do it. A photo whose eyes could not be read (no detector or
  face mesh yet) is tried again next time.

Where the faces come from: the photo's people parse when it has one (found by the parse-ahead index, no model
runs; a run that starts while the worker is still reading the index waits for it, up to 30 s, so a parsed photo
never runs a model because of the order things started in; a parse whose faces have no face mesh, made before
landmarks were kept, does not count: the models run, or the photo stays "not measured" and is tried again), else YuNet (Settings ▸ Faces) and the face mesh alone (`lightcraft_parsing::mesher::Mesher`: the mesh model
only, Core ML when converted, else candle), not the whole parse. Either way on the picture the parse reads
(upright, uncropped, default settings, 2048 px), so the parse's landmarks fall on it. Without a detector or the mesh
file, and no parse, the eyes are unknown and the answer says why (`eyes.unavailable`).

Memory: each photo's reading holds its 2048 px picture (8-bit RGB 3 bytes a pixel, luminance 4, detail maps up to 12:
about 53 MB for 2048 × 1365) after the render, charged to the decode gate (`memory::work_gate`, a quarter of the budget:
384 MiB by default), so the four lanes together hold at most about 210 MB of pictures and wait for each other when the
gate is full; the render itself is counted by its own decode.

How it runs: in the app, `photo.analyze` (menu or control channel) plans on the UI thread (no file read), measures
on up to four worker threads under an activity row that counts the photos and can stop (the models run for one
photo at a time), then files the scores as one undo step. A stopped run changes nothing. The CLI and MCP without an
app run all three at once.

Speed (release, Core ML face mesh, a test Mac, private raws at 2048 px): a 116-photo batch of portraits took 969
ms per photo with the eyes and 479 ms without, so the eyes add about 490 ms per photo (580–670 ms on other batches);
a 297-photo batch 1,206 vs 627 ms. Nearly all of it is the 2048 px render of the raw: a batch whose photos were
already parsed (no model ran) cost the same as one that ran YuNet and the mesh (1,316 vs 1,257 ms per photo).

Known limits: faces turned more than about 3/4 get a low mesh score and read unknown (candle and Core ML agree);
eyes behind glasses read softer than they are (31–35 on one series); a downward look reads 0.13–0.22, near the
closed line; the score sees detail, not expression.

`cargo run --release -p lightcraft-engine --example cull_eyes -- OUT_DIR [--parse K] [--candle] PHOTOS…` measures a
folder and prints every face.

## What's New entry

### Assisted Culling Checks the Eyes
- Photo ▸ Assisted Culling now finds photos where people have their eyes closed or out of focus (focused on an ear or the background), runs in the background with a row you can stop, and offers those photos for rejection afterwards; nothing is rejected for its eyes unless you choose it. The best shot of a burst is now the sharpest one with open, sharp eyes. Filter or make smart albums with Eyes Closed and Eyes Out of Focus.

## Branch

`feat/ai-culling`

## Own files

`crates/pipeline/src/cull/eyes.rs`, `crates/pipeline/src/cull/eyes/tests.rs`, `crates/parsing/src/mesher.rs`,
`crates/engine/src/cull_eyes.rs`, `crates/engine/src/tests/cull_parse.rs`, `crates/engine/src/cmd/cull_run.rs`, `crates/engine/src/cmd/cull_run/tests.rs`,
`crates/engine/examples/cull_eyes.rs`, `crates/ui-egui/src/assisted_culling.rs`,
`crates/ui-egui/src/assisted_culling/tests.rs`, `fork/ai-culling.md`, `fork/tests.d/ai-culling.txt`

## Hooks in official files

- `crates/catalog/src/model.rs`: `Analysis.eyes`, `Eyes`; `rules.rs`: fields `eyesClosed`, `eyesOutOfFocus`;
  `fork_format.rs` `VERSION` 9, `journal.rs` format row, `tests_format_version.rs` `NEWEST_FORK_FIELDS`,
  `tests_fork_format.rs` (fork 9 test)
- `crates/pipeline/src/cull.rs`: `pub mod eyes`
- `crates/engine/src/parse_ahead.rs` (`Known::read_when_loaded`: waits for the index), `tests/parse_ahead.rs` (`library`,
  `settle`, `open`, `fake_parse` made `pub(super)` for `tests/cull_parse.rs`), `tests.rs` (module)
- `crates/ui-egui/locales/*.json`: the dialogs' new strings (English text where not translated yet), the replaced ones removed
- `crates/parsing/src/lib.rs` (`pub mod mesher`), `parser.rs` (`Engine`, `load_mesh_net`, `eye_angle` made
  `pub(crate)`), `face.rs` (`Job::mesh_crop`, `landmarks_of`, shared with `face::run`; a NaN presence drops the points)
- `crates/engine/src/cmd/cull.rs`: `photo.analyze` runs `cull_run`, its help text; `cmd.rs` (`pub mod cull`,
  `cull_run`); `lib.rs` (`pub mod cull_eyes`); `cmd/parsing.rs` (`prepare_plain`, `Rendered::into_parts`)
- `crates/ui-egui`: `lib.rs` (module), `menus/run.rs` and `control.rs` (`photo.analyze` in the background),
  `app/frame.rs` (stop on another library), `state/dialog.rs` (`Dialog::CullSuggest`), `dialogs.rs` (title, button,
  body, the Cull dialog's text), `dialogs/cull.rs` (`cull_suggest_body`), `dialogs/confirm.rs`,
  `tests/dialog_copy.rs`
- `docs/parity-checklist.md` (LR-LIB-CULL, LR-FILT-CULL, LR-IMP-CULL-AT-IMPORT), `docs/target-app-parity.md`

## Tests

`cull` (pipeline), `cull_run cull_parse the_focus_line assisted_culling` (engine), `fork_format format_version rules`
(catalog), `assisted_culling cull_dialog dialog_copy` (ui-egui), `landmarks_need_a_presence` (parsing)
