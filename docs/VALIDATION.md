# Release validation

The automated checks exercise protocol output and input routing; they cannot establish how an
actual terminal decodes images. Release targets are Linux and macOS. Windows, tmux, and SSH are
unverified. Do not infer runtime support from a build-only check.

## Automated gate

Run formatting, Clippy with warnings denied, workspace tests (default/all features), isolated
package tests (no defaults, and transparency without defaults), Rust 1.92 library checking,
rustdoc with warnings denied, the Python harness tests, scripts/smoke.sh, the separate consumer
fixture, and cargo package. CI additionally resolves newest-compatible dependencies to check the
Slint proxy call-order assumption. scripts/check_consumer.py --toolchain 1.92 --check-only checks
fresh consumer MSRV resolution without the sysinfo-based examples.

The consumer fixture exercises identical source under default, no-default, alpha-only,
two-dependency feature unification, and all-feature configurations. It checks the framebuffer
view, default opaque PNG output, explicit alpha opt-in, and the README application. The smoke
matrix covers half-blocks, sixel, Kitty and iTerm2, enhanced key events with an asserted UI change,
focus transitions, terminal-session reinitialization, and panic restoration. The session test is
ignored by ordinary cargo test and is run explicitly inside the fake terminal by smoke.sh.

## Hosted CI — 2026-10-18

GitHub Actions run 39140419410 on commit 1993e00 passed every job: the full gate above on
ubuntu-latest and macos-latest (on macOS, all 27 smoke cases and the six harness tests), the
Rust 1.92 check, and the newest-compatible dependency job. It was the first fully green hosted
run. Earlier runs failed on macOS because a frame-rate bound was tuned on a fast machine, and on
both systems because two tests raced the encoder worker; both were test defects, fixed in c77bad0
and 71002d2.

## Local results — 2026-10-18

macOS local validation passed:

- Workspace tests: 59 passed with defaults, 60 with all features (including doctests), after the
  test fixes above.
- Isolated library-package tests: 51 passed without defaults, 52 with alpha only.
- Formatting and Clippy across all targets/features, with warnings denied.
- Rust 1.92 library check and fresh-consumer resolution; stable consumer output and README checks.
- Six Python harness tests and all 27 pseudo-terminal smoke cases.
- Rustdoc with warnings denied, package verification, archive contents, and the consumer fixture
  against the actual unpacked package.
- Lifecycle and composition regressions in a disposable package directory after updating to
  newest-compatible dependencies, independently of the repository lockfile.

Ordinary test runs intentionally ignore the pty-only session test (covered by smoke.sh) and a
one-line widget rustdoc illustration; the complete application is compiled separately.

## Real-terminal gate

The October 11 measurements in historical PLAN.md predate release hardening. Repeat these cases
on the final implementation, recording exact terminal version, OS, cell/pixel geometry, protocol,
and pass/fail. Do not mark a terminal passed merely because its fake-terminal smoke passed.

| Terminal | Protocol | Version / OS / geometry | Release-hardening result |
| --- | --- | --- | --- |
| Kitty | Kitty | Not recorded for current changes | Pending manual validation |
| Ghostty | Kitty with compression | Not recorded for current changes | Pending manual validation |
| WezTerm | iTerm2 and forced sixel | Not recorded for current changes | Pending manual validation |
| iTerm2 | iTerm2 | Not recorded for current changes | Pending manual validation |
| Terminal.app or another plain terminal | Half-blocks | Not recorded for current changes | Pending manual validation |

For each row: static and animated content; resize/font zoom; overlay open/close (including first
cell occlusion during a Kitty replacement); focus loss mid-drag; Ctrl/Alt/Shift input; popup
transfer; final-pane removal and recreation; normal/error exit and panic restore. Distinguish a
terminal intercepting Shift+click from an event delivered incorrectly by the router. Check
idle output and image cleanup as well as appearance.

Expected overlay behaviour in examples/overlay.rs differs by protocol. Kitty keeps animating
around the overlay box. Sixel and iTerm2 draw the whole image from the first cell, which the box
covers, so the pane stays frozen until `o` hides the box and then animates again; that is a pass.

Run a current RustSec advisory audit before publishing. cargo-audit was not installed in the
local implementation environment; lack of a local audit is not evidence of no advisories.
Publication remains separate from implementation and requires these remaining release checks.
