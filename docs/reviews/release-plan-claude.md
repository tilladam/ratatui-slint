# Claude review of the release hardening plan

Date: 2026-10-18

Claude reviewed the initial draft of [RELEASE_PLAN.md](../../RELEASE_PLAN.md),
the repository, and the installed dependency sources. The CLI was restricted to
Read, Glob, and Grep tools in plan mode, with session persistence disabled and no
MCP tools. It completed successfully without stderr output. This was one review
round; the revised plan incorporates the dispositions below and has not received
a second Claude review. No implementation changes were made as part of this task.

## Original Claude response (verbatim)

I reviewed the plan against the repo, `Cargo.lock` (slint and i-slint-core 1.18.1, ratatui-image 11.1.0, ratatui-core 0.1.2) and those dependency sources. Below, "must-fix" means the plan is wrong or incomplete in a way that would change the implementation; "optional" items are simplifications or extra coverage.

## Must-fix

**1. R1: Slint offers a public way to separate the two quits, so step 1.3's fallback is unnecessary.**
- Explicit `slint::quit_event_loop()` uses one process-wide proxy (i-slint-core `api.rs:1303` → `platform.rs:227`). That proxy comes from the single `new_event_loop_proxy()` call inside `set_platform` (`platform.rs:264-267`).
- The last-window quit goes `WindowInner::hide` (`window.rs:1952-1958`) → `release_keepalive` → `ctx.event_loop_proxy()` (`context.rs:324-330`). That calls `TermPlatform::new_event_loop_proxy()` again and gets a new proxy every time.
- So the fix is: the first proxy handed out (to `set_platform`) quits; every later proxy forwards `invoke_from_event_loop` but ignores `quit_event_loop`.
- Later proxies must still forward invoke, because `spawn_local` gets its proxy the same way (`future.rs:145`).
- This covers every path in step 1.3 at once: Drop, `component().hide()`, the `create_pane` failure path (`src/platform.rs:262`), and reentrancy. It uses only the public `Platform` trait.
- Avoid the deprecated hidden `Platform::set_event_loop_quit_on_last_window_closed`. Its default implementation does `GLOBAL_CONTEXT…window_count += 1`.
- The plan must record that this depends on an undocumented call pattern. The R1 test is what guards it. Add a CI job against the newest compatible dependencies, since `slint = "1.18"` can float.

**2. R1 test design.**
- Runtimes that don't own the proxy (`proxy_tx: None`, `src/platform.rs:211`) never see the last-window quit. A test running in a shared binary would pass without testing anything, so assert `receives_invoke_from_event_loop()` first.
- "Zero, one, several panes" plus explicit quit can't be separate `#[test]`s in one binary. Use one sequential test, or extend `tests/invoke_from_event_loop.rs`.
- The packaged crate includes `tests/**` (`Cargo.toml:12`), so this binary ships too.

**3. R3: the plan doesn't name a mechanism, and the facts are already known.**
- The ForcedWidth branch emits a cell only when `current != previous` (ratatui-core `buffer/diff.rs:133-140`).
- Cell equality covers symbol, fg, bg, modifier, `skip` and `diff_option` (`cell.rs:253-278`).
- ForcedWidth and AlwaysUpdate are alternative variants of one enum, so they can't be combined. That is a fact, not a risk to check.
- AlwaysUpdate falls back to computed symbol width (`diff.rs:149-173`), which goes wrong for a base64 payload.
- Concrete approach: keep an invalidation epoch in the pane and flip a non-rendering attribute (e.g. `fg`) on the payload cell in `Displayed::render` (`src/encode.rs:118-122`). It then differs from the previous buffer for exactly one draw. Verify with CrosstermBackend bytes.
- Document host `terminal.clear()` as the heavy fallback that works today.
- `invalidate()` must also make `update()` return true (set `needs_redraw`). Every example draws only `if pane.update()` (`examples/hello.rs:77`).

**4. Kitty: the invalidation contract needs to be split per protocol (step 4.5 contradicts step 4.2).**
- Kitty fills every cell with a real placeholder symbol using `UNIT_WIDTH`, which is ForcedWidth(1) (ratatui-image `protocol.rs:31`, `kitty.rs:185-187`). So removing an overlay already repaints correctly; add it as a regression and don't change it.
- Retransmitting is impossible without re-encoding. `transmitted` is a private `Arc<AtomicBool>` (`kitty.rs:27,44-51`).
- Recovering lost image storage means resubmitting the retained `pixels` under a fresh id and retiring the old one. That breaks "without reencoding". State it per protocol: raw (sixel/iTerm2) re-emits without re-encoding; Kitty re-encodes.
- Unhandled hazard: Kitty sends the transmit only on the first `render()`. If an overlay covers the first cell in that frame, the transmit is lost for good. With fresh ids plus delete-after-replace, the image vanishes. Add this to the composition contract and tests.

**5. R7: `restore()` writes output before its guard check.**
- `TerminalSession::restore` calls `self.terminal.show_cursor()` unconditionally, before the `ACTIVE` check (`src/terminal.rs:182-185`). So Drop after an explicit restore still emits `ESC[?25h`, possibly into a newer session.
- Gate it on the ownership token as well.
- `ENABLED` and `ACTIVE` are process globals (`src/terminal.rs:85-86`). They must become per-owner state, or be guarded by the token.
- Step 2.3 is mostly done already: `restore_input_modes` attempts every command and keeps the first error, and `restore_terminal` always tries `disable_raw_mode`. Keep it as a regression check rather than new work.

**6. The panic hook conflicts with surviving worker panics (step 5.2).**
- The hook restores the terminal on a panic from any thread (`src/terminal.rs:262-270`). The hook runs before any `catch_unwind`.
- So if an encoder worker panics and the host keeps running "with an error", it will draw into a terminal that has already been restored.
- Also, `submit`'s `lock().unwrap()` (`src/encode.rs:327`) panics the UI thread once the mutex is poisoned.
- The plan must choose one: restore only on panics from the owner thread (record its thread id in the session token), or treat worker panics as fatal.

**7. Error paths still restore before the reader stops.**
- The examples create `rt` before `session` (`examples/hello.rs:42-43`, same in README/lib docs). Drop order is reverse, so on any `?` return or panic the session restores first, then `Runtime::drop` stops the reader. The reader can still consume shell input in between.
- `finish()` only fixes the success path.
- Either create the session before the Runtime, or hand the reader-stop to the session/guard. Then "input reader stops before restore" holds on error paths too.

**8. R5: syncing modifiers for mouse events can deliver fake key events to the wrong pane.**
- Confirmed: modifier state is context-wide and changes only via modifier key events (`window.rs:1268-1273`). `set_active(false)` clears it (`window.rs:1696-1698`), so the plan's "focus first, then sync" order is right.
- But each synthetic `KeyPressed{Shift}` also runs through `process_key_input` to the receiving window's focus item and menubar shortcuts.
- Syncing before a wheel or press on an unfocused pane (`src/input.rs:459-477`) injects Shift/Ctrl key events into that pane's FocusScope.
- Specify the target: the focused pane's window when one exists. Add a test that a FocusScope in an unfocused pane sees no key events.

**9. R4: cancel the capture in the right order.**
- Dispatching `PointerExited` while grabbed ends the grab and makes TouchArea emit `PointerEventKind::Cancel` without `clicked` (`items/input_items.rs:140-148`). That is the correct cancellation.
- But `set_hover` suppresses Exit to a pane that holds the capture (`src/input.rs:503-505`). Clear `captured` first, then send Exit and reset hover; otherwise the cancellation is skipped.

**10. R6 with overlapping panes.**
- The "outside" press/release sent to A uses the real mapped position (`src/input.rs:393-405`).
- If A and B overlap and B wins first-match, that position is inside A, so A gets a real click.
- Use a position that is guaranteed outside A (e.g. `(-1,-1)` logical), and share it with the no-hit path.

**11. Feature gating of new input tests.**
- Most of `tests/input.rs` is `#[cfg(feature = "image-decoders")]`, because std-widgets need decoders.
- New R4–R6 regressions should use builtin elements (`PopupWindow`, `TouchArea`, `FocusScope`), not std-widgets. Otherwise `-p ratatui-slint --no-default-features` silently skips them.

**12. Dependency trimming: the PNG/JPEG/SVG premise holds; the ratatui-image half still needs checking.**
- i-slint-core's `image-decoders` hard-codes `image` with png and jpeg; `svg` pulls resvg.
- `slint/image-default-formats` maps to `image?/default-formats`, i.e. every format (this is why `rav1e` appears in `target/`). Dropping it keeps the promise.
- ratatui-image's `image-defaults` is `image/default`. Confirm nothing in the crate needs it; it only passes `DynamicImage` in.
- Document that `.slint` images in GIF/WebP/BMP etc. will stop loading.

## Ordering

**13.** Move "remove the stable Kitty-ID experiment" (step 5.3) ahead of commit (4). It deletes the `Some(0)` special cases (`src/pane.rs:487,573`, `src/encode.rs:193`) before Kitty invalidation is written against them.

**14.** Treat the R1 proxy mechanism (#1) and the R3 epoch (#3) as the feasibility checks. Both are small and verifiable now, so they don't need to block commits (2)–(3).

## Optional

**15.** `transparency` changes runtime output as well as types (`keeps_alpha`, `src/pane.rs:493-499`). The feature-unification fixture should check encoded output, not just that it compiles.

**16.** The library reads environment overrides of API defaults: `RATATUI_SLINT_SCALE` (`src/pane.rs:115`), `RATATUI_SLINT_PROTOCOL`, `RATATUI_SLINT_KITTY_COMPRESSION` (`src/terminal.rs:139,152`). Decide whether to document or remove them alongside `KITTY_IDS`.

**17.** The `pollster` swap (step 5.7) is not risk-free.
- The current noop-waker loop (`examples/live-preview/src/main.rs:31-40`) keeps polling without needing a wake.
- `pollster` parks until woken, so it deadlocks if the interpreter future ever returns `Pending` without registering the waker.
- Verify that first, or keep the existing loop.

**18.** Document that many terminals take Shift+click for text selection, so R5's Shift pointer gestures may never arrive. Record this in the real-terminal matrix rather than treating it as a failure.

**19.** Sending R4 cancellation as `PointerExited` makes the "unmatched release" case concrete. A release after cancellation currently goes to whatever pane it lands on (`src/input.rs:433-441`); decide whether to drop it.


---

## Dispositions incorporated into the revised plan

The material dependency claims were checked against the installed Slint 1.18.1,
ratatui-core 0.1.2, and ratatui-image 11.1.0 sources. These checks establish the
current implementation behavior, not a promise that upstream internals are stable.
The seven original regressions had already been reproduced; R8 still needs its
own executable reproduction. The following decisions are the plan author's
assessment of Claude's feedback, not additional statements from Claude.

| # | Decision | Change or rationale |
| --- | --- | --- |
| 1 | Accept with caveat | Phase 1 specifies the first-proxy/subsequent-proxy distinction. Public trait usage does not make the call order a supported contract; comments, locked tests, and newest-compatible dependency tests guard that dependency. |
| 2 | Accept | Use one sequential, separate-binary test and assert that its Runtime actually owns the process-wide proxy. Include both explicit quit and invoke delivery. |
| 3 | Accept mechanism; narrow fallback claim | Specify a damage epoch, needs_redraw, and backend-byte checks. Coalesce repeated invalidations so toggling twice cannot erase damage. Do not advertise terminal.clear() as universal recovery without checking protocol behavior, especially Kitty image storage. |
| 4 | Accept | Split raw repainting from Kitty storage recovery, retain working placeholder behavior, and add R8 for overwritten initial transmission. Require a delivery mechanism that observes or bypasses final-buffer occlusion. |
| 5 | Accept | Ownership gates all restore output, including show_cursor. Existing best-effort mode restoration receives regression coverage rather than being presented as new behavior. |
| 6 | Accept | Record the owner thread, restrict automatic restoration to it, and handle worker panics/disconnection without premature terminal teardown or poisoned-lock cascades. |
| 7 | Accept; strengthen panic handling | Share reader-stop control with session restoration and the owner-thread panic hook. Variable declaration order alone does not fix panic-hook ordering. Explicit finish covers normal output flushing; early returns and panic still stop input before restoring modes. |
| 8 | Accept | Specify modifier delivery through the focused window when available; require an explicit no-focus fallback and tests against synthetic keys reaching an unfocused pane. |
| 9 | Accept | Clear router capture before PointerExited so the existing hover suppression cannot swallow cancellation. |
| 10 | Accept | Use a guaranteed out-of-bounds dismissal position and test overlapping panes as well as host-space clicks. |
| 11 | Accept | Use builtin Slint elements for input regressions so isolated no-default-feature runs execute them. |
| 12 | Accept | Verify DynamicImage-based encoding without broad decoder defaults; preserve PNG/JPEG/SVG promises and document the removal of incidental formats. |
| 13 | Accept | Remove stable-ID configuration before changing Kitty composition and invalidation. |
| 14 | Accept | Use the concrete proxy and damage-epoch candidates in small regressions; independent terminal and input work can proceed. R8 retains its own delivery-mechanism gate. |
| 15 | Accept; add explicit policy proposal | Check encoded output as well as compilation under feature unification. The plan additionally proposes opaque defaults and per-pane alpha opt-in so feature capability does not silently change another consumer's output. This policy proposal comes from the integration assessment. |
| 16 | Accept | Retain SCALE, PROTOCOL, and KITTY_COMPRESSION, document their actual precedence, and remove only the broken KITTY_IDS experiment. |
| 17 | Partially accept | Verify interpreter wakeups and use bounded compilation/import/reload checks before switching to pollster. A hypothetical broken Future is not enough reason to retain busy polling indefinitely; defer only for a demonstrated defect. |
| 18 | Accept | Document terminal interception of Shift+click and distinguish absent input from routing defects in the real-terminal matrix. |
| 19 | Accept | Ignore a release belonging to a canceled press until a new press resets that button's state. Cover delayed releases and pane removal. |

The revised plan is ready to guide implementation. It is not a release approval:
regression fixes, R8 reproduction and delivery design, dependency compatibility,
and real-terminal validation remain required work.
