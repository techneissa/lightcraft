# Lessons (orchestrator only; read at session start)

One line each: symptom → cause → guard, at most 250 characters. Keep at most 40 lines: merge lessons or turn them into checks, and prune one when a script, check or skill line guards it. Agents never load this file; a lesson they need goes into their skill or brief as one line.

- A failing test's name was lost (2026-11-02) → the run was piped through `tail -25` → write long runs to a log file in the scratch folder and read the failures from the file.
- No timing tests: a test never asserts how long something took (techneissa, 2026-11-05); wait on a condition instead.
