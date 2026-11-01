# Lens profiles: Mark II lenses

## What it does

Lens profiles: a Mark II lens no longer takes the Mark I profile (fixes too-bright corners on the FE 70-200 F4 II): a lensfun profile for another lens generation is not used.

## Branch

`fix/a7v-lightness`

## Own files

tests in `crates/lensdb/src/tests.rs`

## Hooks in official files

`crates/lensdb/src/lib.rs` (generation check in the profile match, next to the fill-frame hook of `fix/raw-view-colors`); `docs/lens-profiles.md`

## Tests

`lightcraft-lensdb`
