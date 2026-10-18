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

## 2026-10-11 · Stable kitty image id per pane looked frozen in a real kitty · #src/pane.rs #kitty
Situation: ratatui-slint panes with ratatui-image 11.1 StatefulKitty, Unicode placeholders, one image id for the pane's lifetime, every frame re-transmitted under it (a=T, same i=).
Tried: stable id to avoid accumulating images; verified only on ratatui's TestBackend and the fake terminal.
Outcome: user ran `RATATUI_SLINT_PROTOCOL=kitty cargo run --example hello` in kitty, clicked, pressed `+`: "frozen".
Evidence: 2026-10-11 user report. Not yet A/B-tested against the fresh-id path in the same terminal.
Hypothesis: either kitty does not refresh a visible virtual placement when its image is replaced under the same id, or the `+` key never reached Slint (kitty keyboard protocol without REPORT_ALTERNATE_KEYS can report Shift+=). Unconfirmed which.
Next time: suggestion — run sysmon (animates without keys) with RATATUI_SLINT_KITTY_IDS=stable vs fresh in the same kitty before choosing an id strategy. Do not trust TestBackend output for terminal-side image behaviour.
Revisit when: the A/B run shows stable ids refresh correctly.
Cost: one user round-trip; default switched to fresh ids plus delete-after-draw.
Scope: project     Status: active
Result 2026-10-11: user A/B in kitty with sysmon (no keys involved) — fresh ids work, stable ids flicker and draw incorrectly. Stable-id cause confirmed. The frozen `hello` had a second cause: an empty TouchArea inside the FocusScope took the click, so the FocusScope never got focus (reproduced headless, fixed). Lesson: an example that needs input to show change cannot test rendering; test rendering with something that animates by itself.

## 2026-10-11 · Workspace feature unification masked the no-default-features test failure · #Cargo.toml #testing #features
Situation: whole-codebase review, checking the optional image-decoders feature.
Tried: `cargo test --workspace --no-default-features` as evidence for the library's configuration without default features.
Outcome: the workspace command passed, but `cargo test -p ratatui-slint --no-default-features` failed to compile tests/input.rs because generated std-widgets code references the unavailable load_image_from_embedded_data function. The library-only unit tests passed.
Evidence: 2026-10-11 review runs; examples/live-preview depends on ratatui-slint with default features, enabling image-decoders through feature unification in the workspace run.
Next time: verified — test the library package in isolation with `-p ratatui-slint --no-default-features`; a workspace-wide pass does not establish this configuration's support.
Revisit when: the preview dependency disables default features or Cargo's feature-unification behaviour changes.
Cost: an extra test run and correction of the preliminary interpretation.
Scope: project     Status: active

## 2026-10-12 · Delaying scripted pause did not fix the monitor's idle writes · #examples/sysmon.rs #scripts/smoke.sh #testing
Situation: enforcing zero output during a paused system monitor's idle interval.
Tried: suspected the pause key was consumed during startup; moved the key and measurement window one second later.
Outcome: the delayed run still wrote status updates. Temporary frame tracing showed paused=true while the host dirty flag remained true across pane redraws.
Evidence: 2026-10-12 fake-terminal runs and frame trace; `pane_changed || table_dirty.replace(false)` short-circuited before clearing the flag. After clearing it independently and allowing animations to settle, the measured interval wrote zero bytes.
Next time: verified — inspect dirty-state consumption before adjusting scripted timing; side effects that consume a dirty flag must run even when another redraw condition is true.
Revisit when: redraw conditions no longer consume mutable dirty state.
Cost: two diagnostic terminal runs and a temporary instrumented example build.
Scope: project     Status: active

## 2026-10-18 · Slint reexport alone does not support generated macro code · #README.md #testing #api
Situation: release hardening, compiling the README application from a separate consumer crate.
Tried: depended only on ratatui-slint and used its slint reexport for slint::slint!; workspace examples compiled.
Outcome: the separate consumer failed with unresolved `::slint` paths and related generated-code errors. The workspace's direct Slint dependency hid this requirement.
Evidence: 2026-10-18 scripts/check_consumer.py default consumer run; E0433 originates in the Slint macro.
Next time: add an explicitly configured direct Slint dependency to macro consumers and verify the README from a separate Cargo workspace. Direct dependency is required by the generated paths; full consumer validation is tracked with the release checks.
Revisit when: Slint macros support an explicit reexport/crate path or generate paths relative to the invocation.
Cost: a fresh-consumer build and a documentation/test-fixture revision.
Scope: project     Status: active

## 2026-10-18 · Cargo build time consumed the pseudo-terminal test timeout · #scripts/smoke.sh #testing
Situation: expanded smoke matrix ran a terminal-session integration test while fresh-consumer MSRV compilation was also using the target directory.
Tried: launched `cargo test --test session_lifecycle -- --ignored --nocapture` inside the fake terminal's 30-second runtime timeout.
Outcome: the check timed out with zero bytes of child output and no terminal queries; it did not establish a session-lifecycle failure.
Evidence: 2026-10-18 smoke output reported timeout, bytes written 0, and no query replies after all 22 preceding application cases passed.
Hypothesis: Cargo was waiting for the concurrent build's target lock or compiling before starting the test binary.
Next time: compile the test executable before entering the pseudo-terminal, resolve its path from Cargo JSON, then apply the runtime timeout to the executable alone.
Revisit when: the harness separately budgets compilation and program execution.
Cost: one 30-second timeout and a rerun of the terminal-session check.
Scope: global-candidate     Status: active
