# ratatui-slint

Embed [Slint](https://slint.dev) components as pixel widgets inside a
[ratatui](https://ratatui.rs) application.

Slint renders each component with its software renderer into a framebuffer the size of the pane.
The frame is shown through [ratatui-image](https://github.com/ratatui/ratatui-image) as kitty,
sixel, iTerm2 or half-block output. The ratatui loop drives Slint's timers, animations and
rendering, and Slint never runs its own event loop. Mouse and keyboard input goes to the pane
under the pointer or in focus, and keys Slint rejects fall back to ratatui.

Status: pre-release. All headless tests pass, and every example passes scripted checks under a
simulated terminal. It has not yet been tested across real terminals or tmux. See
[PLAN.md](PLAN.md) for the design, measurements and what is still open.

## Usage

```rust
use ratatui_slint::{InputRouter, LoopEvent, PaneInput, Routed, Runtime, SlintWidget};
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};

slint::slint! { export component Hello inherits Window { Text { text: "hello"; } } }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rt = Runtime::new()?;                          // installs the Slint platform
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    rt.spawn_terminal_reader();                            // after the terminal queries
    let mut pane = rt.create_pane(&session.picker, Hello::new)?;
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(session.keyboard_enhanced);
    loop {
        match rt.next_event() {                            // blocks until input, a timer or a frame
            LoopEvent::Terminal(ev) => {
                if router.route(&ev, &[&pane as &dyn PaneInput]) == Routed::Unhandled {
                    // ratatui handles the event; quit on 'q' etc.
                }
            }
            LoopEvent::Quit => break,
            _ => {}
        }
        if rt.begin_frame() {                              // frame cap and idle detection
            let area = session.terminal.size()?.into();
            pane.set_area(area);
            if pane.update() {                             // render + encode only when dirty
                session.terminal.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))?;
            }
        }
        rt.write_pending_output(session.terminal.backend_mut())?; // OSC 52 clipboard
    }
    Ok(())
}
```

Key points:

- **One frame per cycle.** Lay out the panes, call `update`, then draw once. An idle UI makes
  `next_event` block indefinitely and writes nothing to the terminal.
- **Wake-ups from other threads.** `slint::invoke_from_event_loop` and `Runtime::waker` both wake
  the loop.
- **Focus.** Clicking a pane focuses it. Slint keeps Tab and Escape for its own focus chain and
  popups, so Ctrl+G leaves a pane. Change it with `InputRouter::set_exit_key`.
- **Scaling.** `Scale::Fixed(f)` maps one logical pixel to `f` terminal pixels.
  `Scale::Fit { width, height }` fits a fixed-size design, such as a touch panel, into the pane.
- **Encoding.** Sixel, kitty and iTerm2 encode on a worker thread by default, dropping frames it
  cannot keep up with. Half-blocks encode inline. See `SlintPane::set_encode_mode`.
- **Protocol.** It is detected at startup. Override it with
  `RATATUI_SLINT_PROTOCOL=halfblocks|sixel|kitty|iterm2`.
- **Kitty compression.** Set `RATATUI_SLINT_KITTY_COMPRESSION=1` or `TerminalOptions::kitty_compression`
  over slow links. In one measurement it cut kitty output about 55-fold, at some CPU cost per frame.
- **Font zoom.** On `Event::Resize`, pass `terminal::cell_size()` to `SlintPane::set_font_size`
  so panes follow a changed cell size.

Cargo features:

- `image-decoders` (default) decodes PNG, JPEG and SVG images in `.slint` files, including the
  std-widgets icons.
- `transparency` renders with alpha. Kitty and iTerm2 keep it, and the other protocols composite
  over `SlintPane::set_background`.

## Examples

```sh
cargo run --release --example hello            # one pane, smallest complete app
cargo run --release --example sysmon           # process table + animated Slint gauges
cargo run --release --example device_console   # ops console around a live touch panel
cargo run --release -p live-preview -- ui.slint # hot-reloading .slint preview
cargo run --release --example bench            # per-protocol cost, bytes at the writer
```

Use kitty, Ghostty or WezTerm for full quality. Other terminals fall back to sixel or half-blocks.

## Testing

```sh
cargo test --workspace
./scripts/smoke.sh   # every example under a simulated terminal, per protocol
```

`scripts/fake_terminal.py` runs a program in a pseudo-terminal that answers the startup queries
like a kitty, sixel or plain terminal would. It sends scripted clicks and keys, and checks the
final screen.

## Licence

This crate is MIT licensed, see [LICENSE](LICENSE). Slint itself is available under GPL-3.0, a
royalty-free licence or a commercial licence. Users of this crate pick the Slint licence that fits
their product.
