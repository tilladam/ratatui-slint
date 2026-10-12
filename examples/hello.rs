//! The smallest complete app: one Slint pane filling the terminal.
//!
//! `cargo run --example hello`
//!
//! Click the pane, then press `+`/`-`; `q` quits when the pane is not focused (Ctrl+G leaves it).
//! Set `RATATUI_SLINT_PANIC_AFTER_MS=500` to panic mid-run and check that the terminal is restored.

use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
use ratatui_slint::{InputRouter, LoopEvent, PaneInput, Routed, Runtime, SlintWidget};

slint::slint! {
    export component Hello inherits Window {
        in-out property <float> level: 0.3;
        background: #141820;
        // No TouchArea inside: it would take the click, and the FocusScope would never get focus.
        fs := FocusScope {
            key-pressed(e) => {
                if (e.text == "+") { root.level = Math.min(1, root.level + 0.1); return accept; }
                if (e.text == "-") { root.level = Math.max(0, root.level - 0.1); return accept; }
                reject
            }
        }
        Text { x: 16px; y: 12px; text: "Hello from Slint"; color: #e6edf3; font-size: 24px; }
        Rectangle {
            x: 16px; y: 56px; height: 20px; width: (root.width - 32px) * root.level;
            border-radius: 10px; background: #3fb950;
            animate width { duration: 250ms; easing: ease-out; }
        }
        init => { fs.focus(); }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let panic_after = std::env::var("RATATUI_SLINT_PANIC_AFTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis);
    let mut rt = Runtime::new()?;
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    rt.spawn_terminal_reader(); // after the terminal queries, which read stdin
    let mut pane = rt.create_pane(&session.picker, Hello::new)?;
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(session.keyboard_enhanced);
    let started = Instant::now();

    loop {
        if panic_after.is_some_and(|d| started.elapsed() > d) {
            panic!("requested test panic");
        }
        match rt.next_event_capped(panic_after.map(|_| Duration::from_millis(50))) {
            LoopEvent::Quit => break,
            LoopEvent::Terminal(ev) => {
                if let Event::Resize(..) = ev
                    && pane.protocol_type()
                        != ratatui_slint::ratatui_image::picker::ProtocolType::Halfblocks
                    && let Some(font) = ratatui_slint::terminal::cell_size()
                {
                    pane.set_font_size(font); // font zoom changes pixels per cell
                }
                if router.route(&ev, &[&pane as &dyn PaneInput]) == Routed::Unhandled
                    && let Event::Key(k) = ev
                    && k.kind == KeyEventKind::Press
                    && k.code == KeyCode::Char('q')
                {
                    break;
                }
            }
            _ => {}
        }
        if rt.begin_frame() {
            let area = Rect::from(session.terminal.size()?);
            pane.set_area(area);
            if pane.update() {
                session
                    .terminal
                    .draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))?;
            }
        }
        rt.write_pending_output(session.terminal.backend_mut())?;
    }
    drop(pane); // queues deletion of its kitty image
    rt.write_pending_output(session.terminal.backend_mut())?;
    Ok(())
}
