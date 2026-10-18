# Integration and architecture

`Runtime` drives Slint's platform, timers, and wake channel. Each `SlintPane<C>` owns a
component, software window, framebuffer, and encoder. `InputRouter` maps terminal events
into windows. `SlintWidget` places the latest encoded frame in a ratatui buffer.

Keep the runtime, panes, router, and terminal session on one UI thread. A runtime is permanent
for its thread: dropping it does not uninstall Slint's platform. The first runtime in the
process owns Slint's global invoke/quit proxy; later runtimes on other threads do not receive
`slint::invoke_from_event_loop` or `slint::quit_event_loop`. Use `Runtime::waker()` to address a
particular runtime. Native Slint windows and another Slint platform on that thread are not
supported. Hiding or dropping the last embedded pane leaves the host alive.

The quit distinction uses the public Platform trait but depends on Slint requesting its global
proxy before its context-local proxies. A dedicated integration test guards that call order;
CI also tests newly resolved compatible dependencies.

## Frame and output order

1. Complete terminal queries, then start the input reader. The two must not compete for stdin.
2. Read a runtime event and route terminal events with every visible pane. Handle InputError
   as an input failure, separately from an explicit Quit.
3. When begin_frame() allows a frame, lay out all panes and call every pane's update().
   Consume each dirty flag separately; `a.update() || b.update()` skips B when A is dirty.
   Inspect last_error() to report encoding or worker failure. A failed encoder can be replaced
   with a successful set_encode_mode() call; otherwise shut down cleanly.
4. Draw the host and panes once, when any pane or host content changed.
5. Call write_pending_output() with the terminal backend writer after the draw, and also when
   pending clipboard/deletion output exists without a draw. Propagate output failures.

Sixel, iTerm2, and Kitty encode on one bounded worker per pane by default. Each worker keeps
only the newest pending job; layout generations reject stale results. Half-blocks encode inline.
Thread creation is fallible. A worker panic/disconnection wakes the host and becomes last_error();
it does not restore an otherwise running terminal. No async runtime is required.

Slint macro-generated code refers to `::slint`, so macro consumers need the direct, backend-free
Slint dependency shown in README.md; importing only the reexport is insufficient.

## Images and overlays

- Half-blocks are ordinary cells and follow ratatui's diff. Text inside them has low resolution;
  use ratatui text for content that must remain legible without a graphics protocol.
- Sixel/iTerm2 are one escape payload in the first cell, with other covered cells skipped.
  After hiding an overlay, call pane.invalidate(), then update and draw. This wakes an idle
  runtime and retransmits existing encoded data. Repeated calls before drawing coalesce.
  Ordinary unchanged frames send no image payload.
- Kitty uses a fresh image ID for each encoded frame. The upload is extracted from
  ratatui-image's first render on the encoder thread and sent through pending output, separately
  from placeholder cells. A later widget may overwrite any placeholder, including the first,
  without losing the upload. Replaced IDs are deleted after the replacement upload. Always flush
  pending output; merely rendering SlintWidget is insufficient to upload Kitty images.
- Kitty placeholders repaint after an ordinary overlay disappears. If terminal image storage
  is lost, invalidate() creates a fresh encoded image and ID. This costs a new encoding.
- Sixel images are cut to whole six-pixel bands. At the terminal's bottom edge the last cell row
  is omitted to avoid scrolling. Very small areas that cannot hold a sixel band report an error.

The explicit damage contract does not promise automatic detection of arbitrary terminal
clears, overlapping images, or output from other processes. The host controls composition and
must invalidate affected panes. See examples/overlay.rs for an executable overlay toggle.

## Input

Pointer hit testing uses the first matching pane in the supplied slice. A press captures its
pane until release or cancellation. Terminal focus loss cancels the Slint grab without a click;
a delayed release for that canceled press is ignored. A press in another pane dismisses the
old popup before transferring focus. Captured drags continue to the original pane.

Key events go to the keyboard-focused pane. Tab/Escape belong to Slint; Ctrl+G returns focus to
the host by default. Rejected keys return Routed::Unhandled. Legacy terminals receive synthetic
key releases; enhanced keyboard mode uses actual press/repeat/release events. Some terminals
intercept Shift+click for text selection, so that gesture may never reach the application.

Slint modifier state is context-wide and set through key events. Mouse modifiers are therefore
synchronized through the keyboard-focused window. With no focused pane, the pointer target
receives temporary modifier events without acquiring keyboard focus. FocusScope callbacks may
observe these modifier keys. Clipboard copy uses OSC 52; paste uses the local cache and bracketed
paste. Reading the system clipboard back from an arbitrary terminal is unsupported.

PaneInput and PaneRender remain open for wrappers around a SlintPane. Preserve its PaneId,
window, geometry, thread affinity, and output ordering; do not duplicate an identity among panes.

## Shutdown and ownership

TerminalSession reserves one process-wide owner before changing modes. A second active session
is an error. restore() stops attached readers and disarms that session; repeated restore/Drop
cannot write into a later session. Restoration attempts all enabled-mode cleanup and raw-mode
restoration even after a write failure, and returns the first error. A failed restoration still
disarms the session after best effort, since replaying terminal mode pops is unsafe.

On success or an application error, drop panes, then call session.finish(&mut runtime). This
stops input, flushes image deletions and clipboard output, and restores modes even if flushing
fails. Preserve the original application error if cleanup fails too. Drop is emergency cleanup;
it cannot guarantee flushing deletions owned by a still-live pane/runtime.

The panic hook stops input and restores modes only on the terminal owner's thread, before
unwinding. Caught encoder-worker panics do not tear down the terminal. Applications that replace
the panic hook should chain the previous hook. Cleanup during process abort or forced termination
is not guaranteed.

Custom hosts may omit TerminalSession and use their own ratatui backend and input source. Finish
all stdin queries before spawn_terminal_reader(), or feed already-read events through inject().
For shutdown, stop_terminal_reader(), drop panes, write_pending_output(), then restore your modes.
Provide equivalent panic cleanup. When using TerminalSession, start its reader on the same thread
after initialization so the session can stop it during error/panic cleanup.

For several panes, run every update and combine the resulting booleans afterwards. Render each
pane in its layout area and route input with the same areas and the desired first-match ordering.
A resize should also refresh cell_size() and set_font_size() when pixel dimensions are available.

## Features, defaults, and environment

The public Framebuffer view is stable across feature combinations; RGB and premultiplied RGBA
variants and accessors always exist. It borrows row-major physical pixels with dimensions from
pixel_size(). The transparency feature changes storage capability; output remains opaque until
set_transparency(true). Requesting alpha without that feature returns an error. Kitty/iTerm2
preserve opted-in alpha; other protocols composite over set_background().

The default image-decoders feature supports PNG/JPEG/SVG, including std-widgets icons. Optional
feature-only i-slint-core wiring is needed because Slint's public backend-free feature set does
not expose these switches. Broad codec defaults are disabled: GIF/WebP/BMP and other incidental
formats are not promised.

| Environment variable | Precedence |
| --- | --- |
| RATATUI_SLINT_SCALE=auto or a positive number | Replaces the pane's initial default; a later set_scale() wins. |
| RATATUI_SLINT_PROTOCOL=halfblocks/sixel/kitty/iterm2 | Overrides TerminalOptions.protocol and detection; later explicit Picker configuration wins for newly created panes. |
| RATATUI_SLINT_KITTY_COMPRESSION=0/1 (also false/true, no/yes) | Overrides TerminalOptions.kitty_compression when probing; compression still requires terminal support. |

Invalid values fall back to the corresponding default/option. The broken stable-ID experiment
and RATATUI_SLINT_KITTY_IDS have been removed. There is no SSH bandwidth guarantee; measure the
actual geometry and terminal using termbench. tmux passthrough requires terminal/multiplexer
configuration and remains unverified for this release.
