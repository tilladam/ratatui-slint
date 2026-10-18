# Release hardening plan

Date: 2026-10-18

Reviewed with Claude. See the [full review and dispositions](docs/reviews/release-plan-claude.md).

Prepare ratatui-slint 0.1.0 for crates.io by fixing the seven reproduced lifecycle,
input, rendering, and API problems from the project review, investigating the
additional Kitty transmission hazard found during plan review, then tightening
the public contract, dependencies, documentation, and release checks. Preserve
the existing Runtime / SlintPane / InputRouter / encoder separation. This is a
record of the approved plan. Implementation status is recorded below; publication
has not been performed.

The proposed first release supports Linux and macOS. Windows, tmux, and SSH remain
explicitly unverified unless the validation below establishes support. This plan
does not require a new async runtime, a shared encoder pool, partial-image updates,
or an application framework.

## Implementation status — 2026-10-18

The code, examples, documentation, and CI changes are implemented.
All eight findings have regression coverage. The implementation keeps the original module
boundaries and introduces no async runtime or shared encoder pool.

| Area | Implemented result |
| --- | --- |
| Host lifetime | Global/context proxy distinction; sequential owning-runtime regression covers explicit quit, direct hide, factory failure, callbacks, and recreation. |
| Terminal lifecycle | Per-owner state, nested-session rejection, disarmed restoration, attached reader shutdown, owner-thread panic policy, and finish() cleanup in interactive examples. |
| Input | Capture cancellation, delayed-release filtering, pointer modifiers through the focused window, popup dismissal across/overlapping panes, and builtin-only regressions. |
| Composition | Coalesced raw-image invalidation; Kitty uploads extracted on the encoder thread and emitted through pending output independently of overwritten cells; fresh IDs and ordered deletion. |
| Public API | Stable Framebuffer view, opaque defaults with explicit alpha opt-in, fallible thread creation/mode changes, error sources, InputError events, and documented extension traits. |
| Dependencies | Removed broad codec defaults and stable-ID experiment; shared tmux escaping; pollster in live-preview. Lockfile shrank from 672 to 623 packages. |
| Integration | Complete README application, separate consumer/feature-unification fixture, overlay example, integration guide, release notes, historical-plan notice, and expanded CI. |

Validation passed: formatting, Clippy with warnings denied, default/all-feature workspace tests,
isolated no-default/alpha-only tests, Rust 1.92 library and fresh-consumer checks, rustdoc with
warnings denied, six Python harness tests, all 27 smoke cases, package verification, packaged
consumer checks, and lifecycle/composition tests against newly resolved dependencies.

The separate consumer established that Slint macro users still need an explicitly configured
direct slint dependency; importing only the reexport does not compile. Documentation and the
fixture include it. The Kitty delivery design uses the existing pending-output stage, so no new
frame-finalization API was necessary. invalidate() reuses raw payloads and reencodes retained
Kitty pixels without rerendering Slint.

Remaining release gates: fresh real-terminal appearance/interaction validation and a current
RustSec advisory audit (cargo-audit was unavailable locally). Linux CI has been configured but
was not run from this local macOS checkout. See [docs/VALIDATION.md](docs/VALIDATION.md).
No crate has been published and these changes have not been pushed.
Publication remains a separate release action.

## Evidence and constraints

R1–R7 were reproduced in the project review. R8 was identified in dependency
source during Claude's plan review, then reproduced before its implementation fix.

| ID | Priority | Problem | Main location |
| --- | --- | --- | --- |
| R1 | P1 | Dropping the last pane yields LoopEvent::Quit | src/pane.rs, src/platform.rs |
| R2 | P1 | Enabling transparency changes framebuffer()'s public return type | src/encode.rs, src/pane.rs |
| R3 | P2 | Removing an overlay produces no sixel/iTerm2 image repaint | src/encode.rs, src/pane.rs |
| R4 | P2 | Terminal focus loss leaves mouse capture attached to the old pane | src/input.rs |
| R5 | P2 | MouseEvent modifiers do not reach Slint | src/input.rs |
| R6 | P2 | Clicking another pane leaves the previous pane's popup open | src/input.rs |
| R7 | P2 | Dropping a restored session tears down a newer session | src/terminal.rs |
| R8 | P2 | An overlay can overwrite Kitty's one-time transmission cell before output | src/encode.rs, ratatui-image Kitty implementation |

Temporary reproductions from the review are in
`/tmp/ratatui-slint-review-probes/`. They are evidence to adapt, not a permanent
test dependency. Recreate them from the scenarios in this plan if those files
are gone. The final-pane test must run alone in a separate integration-test
binary, because only one Runtime owns Slint's process-wide event-loop proxy.

The baseline passed default/all-features workspace tests, isolated package tests
without default features, Clippy, formatting, Rust 1.92 library checking with the
lockfile, package verification, documentation generation, Python harness tests,
and 13 existing smoke cases. These results do not cover the seven regressions.

Applicable bummer.md entries, with their revisit conditions checked:

- **2026-10-11, smoke tests under script hang:** startup still issues terminal
  queries. Use scripts/fake_terminal.py, not a bare script(1) pty.
- **2026-10-11, plain-text output search misses updates:** assertions about text
  must use the reconstructed final screen; diffed output is still in use.
- **2026-10-11, stable Kitty IDs render incorrectly:** the recorded real-Kitty
  A/B result confirms the failure. Keep fresh IDs and deletion after replacement;
  do not reintroduce stable IDs as an optimization.
- **2026-10-11, workspace feature unification masks failures:** live-preview
  still enables defaults. Test `-p ratatui-slint --no-default-features` in isolation.
- **2026-10-12, delayed input did not fix idle writes:** consume host and pane dirty
  flags independently. Diagnose state before changing smoke-test timing.

## 1. Keep the host alive when panes close

Files: src/platform.rs, src/pane.rs, dedicated lifecycle integration tests.

1. Turn R1 into one sequential regression in its own executable. First assert
   receives_invoke_from_event_loop(), proving this Runtime owns the process-wide
   proxy. Drop the last pane, pump events with a bounded timeout, and prove the
   runtime stays usable. Create and draw another pane afterwards. Exercise
   explicit quit with zero, one, and several panes in the same test.
2. Implement the narrow proxy distinction confirmed in Slint 1.18.1: set_platform
   obtains the first proxy for explicit slint::quit_event_loop(); context-local
   proxy requests handle automatic last-window quit. The first proxy forwards
   quit; subsequent proxies suppress quit but still forward invoke callbacks.
   Preserve the existing behavior of runtimes that do not own the global proxy.
   This uses the public Platform trait but relies on an undocumented call order.
   Record that dependency beside the code; do not use hidden/deprecated Slint
   facilities or run Slint's event loop just for a side effect.
3. Cover direct component().hide(), failed pane construction, callback reentrancy,
   and spawn_local/invoke delivery through the context-local proxies. A blanket
   filter of Quit with zero panes would break explicit quit and is not suitable.
4. Preserve window reference release and stale encoder-result rejection. Keep
   one-time platform/proxy installation as a documented limit. Run this regression
   against locked and newest-compatible Slint dependencies; if the call pattern
   changes, resolve the upstream API or dependency constraint before release.

Acceptance: last-pane closure does not emit host Quit; explicit quit still does
with zero, one, or several panes; recreation of panes works; dropped components
and windows are released; both proxy paths still deliver invoke callbacks. The
test must exercise the proxy-owning Runtime, not pass accidentally on another one.

## 2. Give terminal sessions explicit ownership and reliable teardown

Files: src/terminal.rs, src/platform.rs, examples, terminal-session pty tests.

1. Add an internal session token/generation or equivalent guard. Reserve terminal
   ownership before enabling raw mode. Reject a second active session without
   writing any terminal mode changes. Release the reservation on every setup
   failure.
2. Make restore disarm that session. Drop on an already restored session must
   perform no output and must not touch a newer session. Scope the enabled-mode
   bookkeeping and panic restoration to the active owner. Gate show_cursor()
   too: it currently writes before the global active check. Define the public
   restore_terminal() function as emergency restoration of the current owner;
   an old object's later Drop must not restore a subsequently created owner.
3. Preserve the existing best-effort cleanup after partial writes: try every
   restore command and raw-mode restoration even when an earlier operation fails.
   Add regression checks, keep the first useful error, and document the state
   after a failed restore.
4. Add one explicit shutdown convenience without introducing a new app owner.
   Proposed shape: TerminalSession::finish(&mut self, runtime: &mut Runtime).
   The caller first drops its panes. The helper stops/joins the terminal reader,
   flushes queued image deletions and clipboard output, and restores the terminal,
   attempting restore even when flushing fails. Return the first error. Plain
   Drop remains emergency cleanup; the helper does not pretend to own all panes.
5. Make the reader's stop/join control shareable internally and associate it with
   the active session when a terminal reader is started. Session restoration,
   including Drop and the owner-thread panic hook, stops the reader before
   restoring terminal modes. Take the join handle once, release locks before
   joining, and never join the current thread. Merely reordering local variables
   does not fix the panic path: the panic hook runs before destructors.
6. Limit automatic panic restoration to the recorded terminal-owner thread.
   Catch encoder-worker panics at the worker boundary and surface failure to the
   host without first restoring its terminal. Handle poisoned shared state as a
   worker failure rather than cascading through lock().unwrap(). Custom terminal
   hosts must provide equivalent panic and reader-lifetime handling themselves.
7. Adopt explicit shutdown in every interactive example, including error paths.
   Preserve the original application error if cleanup also fails. Document pane
   disposal, reader stopping, output flushing, and restoration for custom hosts.

Acceptance: reject nested init; repeated restore and Drop are inert; restore old,
init new, drop old leaves new active; setup and shutdown failures restore what
they can; panic restoration remains effective; input reader stops before restore.
Use the fake terminal for actual raw-mode/alternate-screen checks, in addition to
writer-failure unit tests.

## 3. Repair the input state transitions together

Files: src/input.rs, tests/input.rs.

1. On terminal focus loss, cancel pointer capture and hover in addition to
   deactivating keyboard focus and releasing modifier state. Clear the router's
   capture before dispatching PointerExited; the existing hover helper otherwise
   suppresses exit to captured panes. Slint maps exit during a grab to Cancel,
   without clicked. Ignore a later release for the canceled press until a new
   press resets that button's state.
2. Synchronize mouse modifiers before dispatching press, release, move, drag, and
   wheel events. Apply focus transitions first where necessary because window
   deactivation resets Slint modifiers. Keep router bookkeeping consistent with
   Slint's context-wide modifier state. Send synthetic modifier key events through
   the focused pane's window when one exists, since they also reach FocusScope
   callbacks. Explicitly define the no-focused-pane fallback without silently
   transferring keyboard focus on wheel/hover. Preserve legacy synthetic key
   releases and enhanced-keyboard press/repeat/release behavior.
3. Before transferring focus from A to B on a fresh pointer press, let A process
   the outside click so its popup closes, then dispatch the real event to B.
   Use a guaranteed out-of-bounds logical position for A's synthetic dismissal:
   the real pointer may be inside both panes when they overlap. Share that logic
   with clicks outside every pane. Keep captured drags routed to their original
   pane until release/cancellation.
4. Specify overlapping-pane hit order (document current first-match precedence
   unless there is a compelling compatibility reason to change it). Forget
   disappeared panes and reconcile capture, hover, and modifiers consistently.
5. Write the regressions with builtin PopupWindow, TouchArea, and FocusScope so
   they run without image-decoders; avoid hiding them behind the existing
   std-widgets feature gates. Assert an unfocused pane receives no synthetic key
   events while another pane owns keyboard focus.

Acceptance: focus loss mid-drag followed by a click in B reaches B; cancellation
does not click A; Shift/Ctrl/Alt pointer gestures and wheel modifiers are visible
in Slint; moving focus with modifiers held stays correct; popup A closes on clicks
in B or host space; B gets exactly one click; ordinary capture and keyboard
fallback still pass. Include pane removal during interaction and unmatched or
extra mouse-button releases.

## 4. Make image composition and the framebuffer API predictable

Files: src/encode.rs, src/pane.rs, src/widget.rs, tests/output.rs, tests/phase1.rs.

### Raw protocol repainting

1. Preserve the buffer-diff reproduction for both sixel and iTerm2: paint a static
   image, paint a Clear/Paragraph overlay away from its first cell, remove it,
   and assert that the image payload is emitted again. Cover first-cell overlap,
   movement, shrink/grow, hidden panes, and replacement frames too.
2. Provide explicit `SlintPane::invalidate()` semantics for host-driven damage:
   set needs_redraw so update() returns true, wake the runtime, and make the next
   terminal draw retransmit the current raw image without rerendering Slint or
   reencoding unchanged pixels. Keep this distinct from Slint property dirtiness.
   Hosts call it after hiding an overlay or invalidating terminal image contents.
3. Use a damage epoch to change the payload cell's foreground attribute, which
   raw image data does not use, while retaining ForcedWidth(1) and Skip for the
   remaining cells. Cell equality includes this attribute, so the payload becomes
   dirty without changing its measured width. Coalesce repeated invalidations
   before a draw so a two-state toggle cannot cancel pending damage; retain damage
   while hidden. ForcedWidth and AlwaysUpdate are mutually exclusive, and the
   latter computes the wrong width for escape/base64 payloads. Verify actual
   CrosstermBackend bytes, surrounding text styles, and transparency behavior,
   not only TestBackend state.
4. Document the explicit composition contract and update an example to exercise
   opening/closing an overlay. SlintWidget alone cannot see the previous terminal
   buffer or later overlay writes, so avoid promising automatic damage detection
   without an integration mechanism that can actually observe it.

Acceptance: explicit invalidation restores images for supported damage scenarios;
it wakes an idle runtime and requires no Slint state change; ordinary unchanged
frames emit no image payload; steady-state idle output stays zero; sixel still
does not scroll at the terminal's bottom edge. Confirm the visible result in real
sixel/iTerm2 terminals before claiming composition support.

### Kitty transmission and composition

1. Remove the broken stable-ID experiment before changing Kitty invalidation;
   preserve the fresh-ID cleanup regressions and remove its Some(0) special cases.
2. Add a regression for ordinary overlay removal over an already uploaded image.
   Kitty's per-cell placeholders already participate in the terminal diff; avoid
   changing that working path just to match the raw protocols.
3. Define lost-image-storage recovery separately: ratatui-image's transmitted
   flag is private. Recover by reencoding retained pixels under a fresh ID, then
   retire the previous ID after replacement output. The promise of no reencoding
   applies to raw protocols only. Keep the same public invalidation entry point,
   with these protocol-dependent costs documented.
4. Reproduce R8 by covering the first cell on the first frame and on replacement
   frames, then removing the overlay. ratatui-image marks the transmission consumed
   during widget rendering, before later widgets can overwrite that cell. Prove
   the actual output uploads every referenced image and does not retire the last
   usable image before its replacement is delivered.
5. Choose a delivery mechanism after that reproduction: either move transmission
   into an output stage independent of overwritable cells, or inspect the composed
   buffer and acknowledge that the transmission survived. Do not treat render()
   as proof of delivery. If a frame-finalization API or a small upstream change is
   needed, make that integration explicit and exercise it in every example. This
   is an implementation gate; a redraw flag alone cannot solve R8.

Acceptance: ordinary overlays recover; newly encoded images survive first-cell
occlusion; storage-loss invalidation uploads a fresh image; deletion follows
replacement output. Confirm the sequence with backend bytes and real Kitty.

### Stable public framebuffer access

1. Replace the feature-dependent return type with a stable borrowed view, proposed
   as a non-exhaustive Framebuffer<'a> enum with Rgb and PremultipliedRgba variants.
   Both variants and their accessors must exist in every feature configuration;
   only which variant is returned depends on transparency. Keep the internal
   Pixel alias private. Document row order, dimensions, and alpha semantics.
2. If the raw framebuffer has no supported consumer use case, removing the public
   accessor before 0.1 is a smaller alternative; decide before changing tests and
   documentation. Preferred plan: retain diagnostics through the stable enum.
3. Add a small external-consumer compilation fixture whose identical source builds
   with defaults, no defaults, transparency alone, and all features. Include a
   two-dependency feature-unification case so an unrelated user's transparency
   request cannot break the consumer.
4. Separate alpha capability from rendering policy. Proposed 0.1 contract: opaque
   output by default in every feature combination, with an explicit per-pane
   transparency opt-in when supported. Feature unification must not silently
   change a different consumer's visual output. The framebuffer view describes
   its actual storage regardless of output policy. Extend the consumer fixture
   to verify encoded output and explicit opt-in behavior, not compilation alone.

Acceptance: no existing method's type depends on a Cargo feature; feature
unification preserves default output; docs.rs all-features documentation
describes the same core API available to default users.

## 5. Set the public API and dependency policy for 0.1

Files: src/error.rs, src/platform.rs, src/encode.rs, src/pane.rs, src/lib.rs,
Cargo.toml, Cargo.lock, examples/live-preview.

1. Preserve error causes. Use concrete Slint platform/setup error types where
   public and usable; retain factory error context without forcing arbitrary
   Display-only factory errors to implement std::error::Error unless that API
   tradeoff is chosen deliberately. Expose source() for wrapped I/O errors.
   thiserror is optional convenience, not a reason to redesign every error.
2. Distinguish terminal input failure from normal Quit, e.g. a non-exhaustive
   LoopEvent::InputError(io::Error). Update all callers. Make input-reader and
   encoder thread creation fallible and propagate failure through construction
   and encode-mode changes. Build a replacement encoder before discarding the
   working one. Define worker panic/disconnection behavior so a pane cannot
   silently wait forever for a dead worker. Apply the owner-thread panic policy
   from phase 2 so a reported worker error cannot leave a running UI drawing into
   a restored terminal.
3. Finish API cleanup after the stable-Kitty-ID removal in phase 4: remove unused
   configuration storage and update related documentation. Remove KITTY_IDS only;
   retain and document SCALE, PROTOCOL, and KITTY_COMPRESSION overrides and their
   precedence relative to constructors, defaults, and later explicit setters.
4. Mark extensible error/event/scale/configuration types non-exhaustive where
   appropriate. Preserve construction through Default plus setters/builders.
   Make an explicit decision about sealing PaneInput and PaneRender: default to
   keeping purposeful extension points open, with their contracts documented;
   avoid exporting implementation-only traits merely for internal dispatch.
5. Remove ratatui-image/image-defaults and Slint/image-default-formats from the
   unconditional feature list. Keep PNG for terminal output and the formats
   promised by image-decoders (PNG/JPEG/SVG) working. Offer a separate opt-in
   broader-format feature only if justified. Use cargo tree to confirm the
   resulting default and no-default graphs; do not claim byte/build-time savings
   without measurement. Retain the feature-only i-slint-core dependency until
   Slint exposes the necessary switches publicly. Verify ratatui-image needs no
   extra decoding features when passed DynamicImage, and document the removal of
   incidental GIF/WebP/BMP and other unpromised .slint image formats.
6. Consolidate tmux passthrough escaping used by clipboard, Kitty deletion,
   sixel, and iTerm2 in a private helper. Preserve protocol-specific chunking and
   terminators. Keep the custom sixel/iTerm2 workarounds and their rationale.
7. Replace live-preview's noop-waker polling executor with pollster::block_on as a
   dependency of that example package only, after checking the interpreter's
   default compilation/import futures register wakeups correctly. Exercise initial
   compilation, imports, errors, and reload with bounded timeouts. Pollster parks
   between wakeups, unlike the current busy loop. Keep the simple watcher;
   file-watcher libraries and asynchronous compilation are not needed for this
   release. If an actual wakeup defect is found, resolve it or defer this swap
   with the concrete reproduction recorded.

Acceptance: all callers handle the revised errors; thread creation failure leaves
existing state valid; dead workers surface an error and wake the host; the default
decoder promise remains true after trimming; the library still checks on Rust
1.92. Avoid expanding this phase into generic async/event-loop infrastructure.

## 6. Publish an accurate integration guide and test matrix

Files: README.md, src/lib.rs rustdoc, examples, scripts/fake_terminal.py,
scripts/test_fake_terminal.py, scripts/smoke.sh, .github/workflows/ci.yml,
Cargo.toml, current architecture/limitations documentation.

1. Supply a copyable Cargo.toml snippet and complete smallest application, using
   the Slint reexport or an explicitly configured direct dependency. Implement
   quit handling, resize/font-size updates, pending-output flushing, and shutdown.
   Exercise the documented code from a fresh consumer crate so the repository's
   dependency/feature unification cannot conceal missing imports or features.
2. Explain ownership and call ordering, thread confinement, platform permanence,
   the process-wide proxy, per-runtime Waker, unsupported native windows, clipboard
   limitations, input focus/exit keys, raw-image invalidation, bottom-row sixel
   cropping, feature choices, alpha opt-in, environment override precedence, and
   half-block text limitations. Include a multi-pane/custom-host integration
   example or focused recipe. Explain that terminals may intercept Shift+click
   for text selection before the application sees it.
3. Mark PLAN.md as historical and point it to current architecture and limitations
   documentation. Avoid rewriting the historical measurements. Make all README
   links resolve on GitHub and from the packaged crate, using appropriate absolute
   links or including referenced documentation. Keep the prototype excluded from
   the package. Add a short 0.1 release/change note.
4. Extend the fake terminal to simulate iTerm2 output and enhanced keyboard
   negotiation. Assert enhancement push/pop, actual enhanced key events, and
   nontrivial UI changes; detecting any graphics payload is not proof that input
   worked. Add focus-loss and session-reinitialization scenarios. Keep ordinary
   text assertions on the reconstructed screen.
5. Add the regression/consumer checks and package/docs checks to CI. Run library
   MSRV checks independently of sysinfo-based examples. Choose a supported-OS
   statement consistent with the Linux/macOS CI matrix; Windows stays unverified
   until it has a meaningful build and runtime check. Add newest-compatible
   dependency testing in a disposable checkout/lockfile, especially for the Slint
   proxy call-order regression; keep locked dependency tests as well.
6. Record real-terminal results with terminal/version, OS, protocol, geometry, and
   pass/fail. Cover Kitty, Ghostty, WezTerm, iTerm2, and a half-block terminal,
   including resize/font zoom, static and animated content, overlays, focus loss,
   modifiers, popup transfer, final-pane removal, and exit/panic restore. Record
   intercepted gestures separately from delivered-but-misrouted input. A local
   fake-terminal pass cannot stand in for terminal-side display behavior.
7. tmux and SSH can remain explicitly experimental/unverified for 0.1. If they are
   advertised as supported, add real passthrough and remote checks, including
   image deletion and clipboard behavior. Do not invent a bandwidth guarantee.

## Implementation order and release gate

Use reviewable commits: (1) host lifetime, (2) terminal ownership/shutdown,
(3) input transitions, (4) stable-ID removal followed by composition/invalidation,
(5) framebuffer contract and alpha policy,
(6) errors/API cleanup, (7) dependency/helper cleanup, (8) docs and expanded CI.
Pair each functional fix with its regression. API decisions in phases 4–5 must
settle before finalizing examples and consumer documentation. Verify the concrete
Slint proxy and raw-protocol damage-epoch mechanisms with small regressions;
independent terminal/input work need not wait for them. R8's delivery mechanism
must be settled before claiming robust Kitty composition.

Required automated checks on the final tree:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
cargo test -p ratatui-slint --no-default-features
cargo test -p ratatui-slint --no-default-features --features transparency
cargo +1.92 check -p ratatui-slint --lib --all-features --locked
cargo doc -p ratatui-slint --all-features --no-deps
python3 -m unittest discover -s scripts -p 'test_*.py'
./scripts/smoke.sh
cargo package -p ratatui-slint
```

Also run the external consumer/feature-unification fixtures, check MSRV resolution
from a fresh consumer rather than only the repository lockfile, inspect the final
archive and links, and make rustdoc warnings fatal in CI. Use a current advisory
check when available; report any unperformed check rather than imply it passed.

Release criteria: R1–R7 fixed with regressions and R8 reproduced and resolved (or
disproved with a permanent regression); default and transparency consumers share
a stable API and predictable default output; shutdown examples restore ownership
correctly on success, errors, and owner-thread panic; declared
terminal support matches real validation; package/docs checks pass; outstanding
limitations are visible in the public documentation. Publication remains a
separate action after implementation and review.

At task boundaries, apply the append-only bummer.md rules for actual rejected or
costly failed approaches. Do not use the negative-knowledge log as a bug backlog.
