# bummer.md — what did not work here and why.
# Append-only during tasks; the `bummer` skill merges, promotes and retires.
# Template per entry:
#
# ## YYYY-MM-DD · title · #tags/paths
# Situation: task, constraints, versions
# Tried: what seemed reasonable / what was proposed
# Outcome: what failed or was insufficient (observed, not inferred)
# Evidence: dated observation or user correction, reference if any
# Hypothesis: suspected cause (optional; keep separate from Outcome)
# Next time: verified alternative, or suggestion — say which
# Revisit when: conditions that would invalidate this
# Cost: what the dead end cost (time, reverts, user corrections)
# Scope: project | global-candidate     Status: active

## 2026-10-11 · Smoke-testing TUI examples under `script` hangs · #examples #scripts/smoke.sh #testing
Situation: running examples/sysmon headless to check startup, quit and terminal restore; crossterm 0.29, ratatui 0.30, ratatui-image 11.1.
Tried: `(sleep 4; printf q) | script -q log sh -c 'stty rows 40 cols 120; ./sysmon'`.
Outcome: the program never drew a frame and never quit; the log ended after the cursor-position query (`ESC[6n`). Killed after 60 s.
Evidence: 2026-10-11, recorded output was 122 bytes ending in `ESC[?u ESC[c ESC[6n`.
Hypothesis: nothing in a `script` pty answers terminal queries, so startup stalls on the unanswered queries and the scripted `q` is consumed by a query read.
Next time: verified — use scripts/fake_terminal.py, which answers device attributes, cell size, cursor position and kitty queries.
Revisit when: the startup sequence stops issuing blocking queries.
Cost: one hung run and a minute of timeout.
Scope: global-candidate     Status: active

## 2026-10-11 · Plain-text search of TUI output misses updated values · #scripts/fake_terminal.py #testing
Situation: asserting on screen text in recorded output of a ratatui app.
Tried: stripping escape sequences from the byte stream and searching for an expected string such as "target 25.0°C".
Outcome: the check failed although the screen showed the text: ratatui's diff rewrote only the changed cells, so the stream held just the changed digit.
Evidence: 2026-10-11, device_console readout check failed until the harness replayed output on a character grid.
Next time: verified — check against the last screen reconstructed from cursor moves (`last_screen` in scripts/fake_terminal.py), not the raw stream.
Revisit when: never for diffing renderers.
Cost: two test iterations.
Scope: global-candidate     Status: active
