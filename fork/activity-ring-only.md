# Activity ring only

List: Every progress indicator shows in the top-bar ring; the corner card stack is gone (issue #295). Branch `feat/295-ring-only-progress`.

## What it does

- A task that runs, whether the app started it for the photo in front of you (`background`) or a person did (import, export, scans, merge, Find Missing Photos…), shows in the top-bar ring and nowhere else. No card in the top-left corner for any task.
- The ring fills with the combined progress of all running tasks (the mean of those that know their total) and sweeps while none does. A task shows once it is 0.5 s old, so quick ones don't flash; with none due there is no ring.
- Hovering the ring lists each task (name, what it is on, percentage, count). A batch's line also has the time left and the failures ("AI Denoise · 37 of 200 · about 6 min left · 2 failed"); its per-photo tasks are not listed. A click opens the same list with ✕ for every task that can stop, which runs `activity.cancel`.
- The ring's accessible name is "Running tasks" (it was "Preparing photos in the background").
- Unchanged: quit asks first while a task that can stop runs; `activity.list`, `activity.cancel`; `ui.inspect`'s `activity`; `TaskInfo::background` (it only tells now which tasks the app started by itself). Callers still choose `start` or `start_background` as before. The message when a batch of several photos ends ("AI Denoise: 198 done, 2 failed: why") stays.

## Branch

`feat/295-ring-only-progress`, from `custom`.

## Own files

`fork/activity-ring-only.md`, `fork/tests.d/activity-ring-only.txt`

## Hooks in official files

- `crates/ui-egui/src/panels/activity.rs`: no longer draws anything (the stack, its rows, bars, fold and "+N more" are removed); it keeps the words the ring shares (`count_text`, `eta_text`, `summary_text`), `cancel`, the end-of-batch message, and `poll` (called each frame from `app/frame.rs`, in place of `show`), which also asks for a redraw while a task runs so a young task reaches the ring.
- `crates/ui-egui/src/panels/activity_ring.rs`: `tasks` no longer filters on `background`; `line` adds a batch's time left and failures; the accessible name.
- `crates/ui-egui/src/lib.rs`, `app.rs`: the `activity_expanded` and `activity_open` fields are removed.
- `crates/ui-egui/src/tests/activity.rs`, `offline.rs`, `headless/tests.rs`, `progress.rs`: tests that drew or clicked a corner row now use the ring (`icon:activityRing`, then `activity:cancel:<id>` in its open list); the tests of the stack's layout (overflow, width, cross position, growing, fold) and of the activity bar's accessible name are removed with the stack.
- `docs/background-tasks.md`: a fork note at the top. `docs/control-protocol.md`: the `ui.inspect` `activity` sentence.

## Tests

`fork/tests.d/activity-ring-only.txt`:

- `lightcraft-ui-egui` (`tests::activity`): the ring appears after half a second and ✕ in its list cancels; it lists an export (not background) as well as background work and goes when they end; a task that can't stop has no ✕; a batch is one line with the time left and failures; model work names what it is on; quit with a running task asks first; the import and scan ✕ tests (`import_reads_files_off_the_ui_thread_and_can_be_cancelled`, `folder_scan_cancel_and_busy`) reach their ✕ through the ring.

## What's New entry

### Progress in one place
- Every progress indicator, such as reading a folder, importing or exporting, now shows in the ring in the top bar. The cards in the top-left corner are gone. Hover the ring to see each task and its count, click it to stop one.
