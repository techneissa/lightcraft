# Shared SAM3 model folder

## What it does

LightCraft looks for the SAM 3 model (AI masks) in the folder it shares with PhotoCraft, `~/Library/Application Support/ArtCraft/models/sam3/` (`model.safetensors`, `vocab.json`, `merges.txt`), before its own `<config>/models/sam3`. `<config>/models/sam3` is a symlink to the shared folder, so the old place keeps working. Mac only.

Lookup order (`default_sam3_dir`):
1. A non-empty `LIGHTCRAFT_SAM3_DIR`.
2. The shared folder, when it holds the model.
3. `<config>/models/sam3`, when it holds the model.
4. With neither, the shared folder: a download goes there, never into a second 3.4 GB copy, and never replaces the symlink with a real folder. The download only creates the folder (`create_dir_all`) and writes `.part` files that it renames inside it; it never deletes or replaces the folder or a symlink to it.

`sam3-mirrors.txt` stays in `<config>/models/`.

Rules (techneissa, 2026-11-01):
1. Never move the model back, delete the ArtCraft folder, or replace the symlink with a real folder.
2. Any new model both apps could use goes in `ArtCraft/models/<model-id>/`.

## Branch

`feat/16-shared-sam3`

## Own files

`crates/engine/src/config/shared_sam3.rs` (the lookup and its tests)

## Hooks in official files

`crates/engine/src/config.rs`: `mod shared_sam3;`, `default_sam3_dir` calls `shared_sam3::sam3_dir` (the old private `sam3_dir` is gone), and the official test `sam3_dir_prefers_the_override` passes `None` for the shared folder through a local closure of the same name. `docs/whats-new.md` (one line).

## Tests

`cargo test -p lightcraft-engine --lib config::` (the download tests, in `config::shared_sam3`, need `--features sam`)
