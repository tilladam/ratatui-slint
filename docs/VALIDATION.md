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
and 71002d2. Run 39152078126 on 041fe12, the build validated in real terminals below, also passed
all four jobs.

## Local results — 2026-10-18

macOS local validation passed:

- Workspace tests: 60 passed with defaults, 61 with all features (including doctests), after the
  test fixes above and the Kitty upload-order fix (157a5c8).
- Isolated library-package tests: 52 passed without defaults, 53 with alpha only.
- Formatting and Clippy across all targets/features, with warnings denied.
- Rust 1.92 library check and fresh-consumer resolution; stable consumer output and README checks.
- Seven Python harness tests and all 27 pseudo-terminal smoke cases, which now also check that
  every Kitty placeholder follows its image's upload.
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

Results of 2026-10-18 on macOS 27.0.1, build 041fe12, all with release builds of the examples.
Cell sizes and fps are what termbench printed; geometry is cells across × down.

| Terminal | Protocol | Version / OS / geometry | Release-hardening result |
| --- | --- | --- | --- |
| Kitty | Kitty | 0.49.2, macOS 27.0.1; 17×33 px cells, 152×51, 21.6 fps | Pass |
| Ghostty | Kitty | 1.3.1, macOS 27.0.1; 16×34 px cells, 168×53, 59.9 fps | Pass |
| WezTerm | iTerm2 and forced sixel | 20240203-110809-5046fc22, macOS 27.0.1; 17×37 px cells, 150×47, 60.1 fps | Pass, including forced sixel |
| iTerm2 | iTerm2 | 3.7.6, macOS 27.0.1; 7×17 cells reported, 184×55, 58.6 fps | Pass; renders at about 1× on Retina |
| Terminal.app | Half-blocks | 2.15, macOS 27.0.1; 9×18 cells reported, 142×48, 60.1 fps | Pass |

How each row was run: Ghostty by hand. Kitty through its remote control, with real key events
from kitty but mouse input sent as SGR reports, so Kitty's own mouse handling was not exercised:
Shift+click interception and mouse input after exit were not checked with a real mouse there.
WezTerm, iTerm2 and Terminal.app with real macOS mouse and keyboard events, with window
screenshots judged by eye and animation judged by comparing consecutive captures. In each, focus
loss mid-drag was a real application switch. Linux terminals were not run in this session.

On 2026-10-19 a tester reported WezTerm on Linux working, smooth and with working input. They
also reported that Konsole auto-detects half-blocks and works with sixel when forced with
`RATATUI_SLINT_PROTOCOL=sixel`. A second tester also reported Linux working. Versions,
distributions, geometry and the cases run were not recorded, so these reports have no table rows.
Konsole gets half-blocks because ratatui-image 11.1 excludes sixel and kitty whenever
`KONSOLE_VERSION` is set, citing Konsole's sixel bugs; its README calls the 24.12 fix incomplete.

Findings, none of them a library defect:

- This validation found a Kitty regression from release hardening: each new frame's
  placeholders reached the terminal before its upload, so continuously animated panes stayed
  blank. Fixed in 157a5c8; the fake terminal now checks the order.
- WezTerm takes Shift+click for its own selection (its default bypass modifier), so the app never
  sees it. iTerm2 bypasses with Option, and there and in Terminal.app Shift+click reaches the app.
- iTerm2 reports cell sizes in points. On a Retina display Slint therefore renders its images at
  about 1× and iTerm2 scales them up; text is visibly softer than in WezTerm. Terminal.app also
  reports points, but its half-block output is unaffected.
- Terminal.app's default keymap sends a plain Left for Option+Shift+Left and `ESC b` for
  Option+Left, so word selection with Option does not work there unless the keymap is changed.
- In iTerm2 the first synthetic keystroke after a program starts did not arrive, also for a bare
  raw-mode reader without keyboard flags. Not seen in other terminals; unconfirmed with a real
  keyboard. Later keys arrived normally.

Not covered by any example and therefore checked by automated tests only: popup transfer between
two panes, removal and recreation of the final pane, and error exit.

For each row: static and animated content; resize/font zoom; overlay open/close (including first
cell occlusion during a Kitty replacement); focus loss mid-drag; Ctrl/Alt/Shift input; popup
transfer; final-pane removal and recreation; normal/error exit and panic restore. Distinguish a
terminal intercepting Shift+click from an event delivered incorrectly by the router. Check
idle output and image cleanup as well as appearance.

Expected overlay behaviour in examples/overlay.rs differs by protocol. Kitty keeps animating
around the overlay box. Sixel and iTerm2 draw the whole image from the first cell, which the box
covers, so the pane stays frozen until `o` hides the box and then animates again; that is a pass.

Run a current RustSec advisory audit before publishing. Last run on 2026-10-19 at 13a964b with
cargo-audit 0.22.2 and 1338 advisories: no vulnerabilities, exit status 0, and four informational
`unmaintained` warnings for bincode 2.0.1 (RUSTSEC-2025-0141), zbus_names 4.3.4
(RUSTSEC-2026-0371), zvariant 5.15.0 (RUSTSEC-2026-0370) and zvariant_derive 5.15.0
(RUSTSEC-2026-0372). None of the four is compiled. `cargo tree -e all -i` finds no path to them
for Linux, macOS, Windows or wasm32 in this workspace, and none for `--target all` in a fresh
consumer with a path dependency. They are Cargo.lock entries for unused optional dependencies in
Slint's dependency tree, so a consumer's audit reports the same four advisories. Removing them
needs a Slint release on zbus 6 without bincode; repeat the audit after each Slint upgrade.
Publication remains separate from implementation and requires these remaining release checks.
