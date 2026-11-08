# All AI models in the shared ArtCraft models folder

List: every AI model on the Mac (faces, denoise too) lives in `ArtCraft/models/<model-id>/`, shared with PhotoCraft; the old folders are read and moved once at start. Branch `feat/56-shared-models`.

## What it does

On macOS every model is in `~/Library/Application Support/ArtCraft/models/<model-id>/`, so LightCraft and PhotoCraft never keep two copies. SAM 3, LaMa, BiRefNet and the reflection models were there already; now the face models (`auraface-v1`, `yunet-2023mar`, `sface-2021dec`) and the denoise models are too. Off the Mac nothing changes.

- **One function** for the folder: `shared_models::shared_models_dir` (`crates/engine/src/shared_models.rs`). SAM 3, LaMa, BiRefNet, reflections, faces and denoise all ask it.
- **Lookup** (`ModelRoots`): the shared folder first, then the old one (`<config>/models`, `<config>/denoise-models`). The old folder still holds `settings.json`, the user's `catalog.json` and the download staging folder. New downloads and installs go to the shared folder, unless the model is already in the old one. A model is never downloaded when it is in either folder. `LIGHTCRAFT_FACE_MODELS` and `LIGHTCRAFT_DENOISE_MODELS` still pick one folder and switch the shared one off for that kind.
- **Moved once at start** (the app, on a worker thread: `shared_models::migrate_old_models`; `crates/engine/src/shared_models/migrate.rs`; as PhotoCraft does): a model LightCraft knows (its published size and SHA-256) in the old folder is checked, then its files are moved into `ArtCraft/models/<model-id>/` (`model.onnx` by rename, nothing downloaded; the few small files beside it, `face-model.json`/`denoise-model.json` and `installed.json`, copied first). A checked copy already in the shared folder is never touched and only the old duplicate goes (a shared copy that fails the check is replaced by the checked old one). A failed move leaves both copies, logs why, and the app starts. A link (or anything but a plain file) at the shared path is not a checked copy: both stay and it is logged. A `model.onnx.part` older than an hour is cleared as a crashed install; a newer one holds the move back. The move shows one row, "Moving AI models to the shared folder", in the activity stack. A model added by hand (no published hash) stays in the old folder and is still found there. `ArtCraft/models/sam3` and the `sam3` link in the old folder are never touched.
- **Remove** on a model in the shared folder: the button's second click reads "Remove for Every ArtCraft App?", with no help line (a model only LightCraft keeps goes with one click, as before). `faces.models.remove` and `denoise.models.remove` refuse a shared model unless they get `"everyApp": true`. The model lists carry `shared` per model and `sharedDir`.

Tests use temporary folders only, never the real `ArtCraft` or `LightCraft` folders; the folders are parameters (`ModelRoots`, `migrate_old_models_in(home, faces_old, denoise_old)`).

## Branch

`feat/56-shared-models`

## Own files

`crates/engine/src/shared_models.rs`, `crates/engine/src/shared_models/migrate.rs`, `crates/engine/src/tests/shared_models.rs`, `crates/ui-egui/src/model_remove.rs`

## Hooks in official files

`crates/engine/src/lib.rs` (`pub mod shared_models`, `Session::face_shared_dir`), `crates/engine/src/config.rs` (`default_face_shared_dir`, `default_denoise_shared_dir`, `Session::face_roots`), `crates/engine/src/config/shared_sam3.rs`, `crates/engine/src/lama_fill.rs`, `crates/engine/src/segment/birefnet.rs`, `crates/engine/src/reflections.rs` and `reflections/shared_models.rs` (the folder comes from the one function), `crates/engine/src/cmd/face_models.rs`, `face_detect.rs`, `face_recognize.rs`, `cmd/denoise.rs`, `denoise/{settings,session,state,model}.rs` (lookup in both folders, `everyApp`), `crates/ui-egui/src/dialogs/faces.rs` and `panels/denoise.rs` (the Remove button), `crates/ui-egui/src/lib.rs` (`mod model_remove`), `apps/lightcraft/src/main.rs` (the move at start), `apps/lightcraft/src/session.rs` (`open_session` uses `with_default_face_models`), `crates/engine/src/activity.rs` (a doc line), the `mod` line of `crates/engine/src/tests.rs`, and `tests/denoise.rs` (two helpers made `pub(super)`).

## Tests

```bash
cargo test -p lightcraft-engine --lib -- shared_models
cargo test -p lightcraft-ui-egui --lib -- model_remove
```

## What's New entry

### One models folder for every ArtCraft app
Face and denoise models now live in the folder LightCraft shares with PhotoCraft, so a model is downloaded once. Models already in LightCraft's own folders move there at the first start. Removing a shared model asks "Remove for Every ArtCraft App?".
