# Performance: XMP sidecar writes

## What it does

Performance: XMP sidecar writes. With Settings ▸ Import ▸ "Automatically write changes into XMP sidecars" on, an edit no longer writes the sidecars itself: the UI thread gathers each photo's packet and paths, one worker merges and writes them in order under a "Writing XMP sidecars" activity row (shown once the batch takes more than a moment), and a newer write for a photo replaces its waiting one. Each result comes back on the frame: a failed write leaves the photo unsaved and the 10 s retry queues it again. Save and Read Metadata, import and Synchronize Folder scans, rename and move, undo of file moves, opening or closing a library and quitting land the queue first, so a queued write never lands after a newer one. Without the queue (tests, CLI, MCP) the writes stay in the command.

## Branch

`perf/sidecar-writes`

## Own files

`crates/engine/src/sidecar_queue.rs`, `crates/engine/src/tests/sidecar_queue.rs`, `crates/ui-egui/src/tests/sidecar_queue.rs`

## Hooks in official files

engine `sidecar.rs` (`sidecar_target` shared by the queue and `save_sidecar_with`; auto-write queues; `save_sidecar`, `read_sidecar_op_with_warnings` and the retries wait; `queue_unsaved_sidecar_retries`), `lib.rs` (field, module; `apply_with_files` waits), `import/scan.rs`, `import/job.rs`, `rename/plan.rs`, `library.rs` (wait first), `tests.rs`; UI `app/frame.rs` (poll, queued retry), `headless.rs` (`busy`), `tests.rs`; app `session.rs` (queue on)

## Tests

`sidecar_queue` (engine and ui-egui), `xmp`, `sidecar`, `sync`, `rename`, `import`
