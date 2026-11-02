# Lessons (orchestrator only; read at session start)

One line each: symptom → cause → guard, at most 250 characters. Keep at most 40 lines: merge lessons or turn them into checks, and prune one when a script, check or skill line guards it. Agents never load this file; a lesson they need goes into their skill or brief as one line.

- A failing test's name was lost (2026-11-02) → the run was piped through `tail -25` → write long runs to a log file in the scratch folder and read the failures from the file.
- Three lanes building at once (2026-11-03) pushed the Mac's load to ~155 on 12 cores; a 6 min check took 35 min → each lane cold-builds its own target dir → run one building lane at a time; queue the rest.
