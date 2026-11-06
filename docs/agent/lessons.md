# Lessons (orchestrator only; read at session start)

One line each: symptom → cause → guard, at most 250 characters. Keep at most 40 lines: merge lessons or turn them into checks, and prune one when a script, check or skill line guards it. Agents never load this file; a lesson they need goes into their skill or brief as one line.

- A failing test's name was lost (2026-11-02) → the run was piped through `tail -25` → write long runs to a log file in the scratch folder and read the failures from the file.
- A bench's "after" run reran the "before" binary (2026-11-07) → cargo in a shared target reuses another worktree's build when your files are older → check the log says `Compiling`, or `touch` the crate sources first.
- No timing tests: a test never asserts how long something took (techneissa, 2026-11-05); wait on a condition instead.
- Fork CI red on a test the Mac never compiles (2026-11-05) → a signature change missed a `cfg(not(target_os = "macos"))` caller → after changing a signature, grep its callers inside `cfg(not(...macos))` blocks too.
