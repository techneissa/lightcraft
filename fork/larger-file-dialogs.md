# Larger file dialogs

## What it does

Taller, resizable open and save windows on macOS (at least 520 pt).

## Branch

`feat/larger-file-dialogs`

## Own files

`crates/filepanel` (the second crate allowed `unsafe`)

## Hooks in official files

`apps/lightcraft/src/platform.rs` (1 call in `file_dialog`), app and workspace `Cargo.toml`, `xtask/src/layers.rs`, `AGENTS.md` / `CLAUDE.md` (the `unsafe` line)

## Tests

`lightcraft-filepanel`
