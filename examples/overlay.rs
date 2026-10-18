//! Toggle a ratatui overlay over an animated Slint pane: `o` toggles, `q` quits.
//! Demonstrates explicit sixel/iTerm2 damage and Kitty uploads under first-cell occlusion.
//! Sixel and iTerm2 draw the image from the pane's first cell, which the overlay covers, so
//! those panes freeze until the overlay is hidden; Kitty keeps animating around it.
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::Rect,
    widgets::{Clear, Paragraph},
};
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
use ratatui_slint::{InputRouter, LoopEvent, Runtime, SlintWidget, slint};

slint::slint! {
    export component Demo inherits Window {
        background: #202830;
        Rectangle {
            x: Math.mod(animation-tick() / 20ms, 100) * 1px;
            y: 40px; width: 50px; height: 30px; background: #40c080;
        }
        Text { text: "o: toggle overlay / q: quit"; color: white; }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = Runtime::new()?;
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    runtime.spawn_terminal_reader()?;
    let mut pane = runtime.create_pane(&session.picker, Demo::new)?;
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(session.keyboard_enhanced);
    let mut overlay = true;
    let mut host_dirty = true;
    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        loop {
            match runtime.next_event() {
                LoopEvent::Quit => break,
                LoopEvent::InputError(e) => return Err(e.into()),
                LoopEvent::Terminal(event) => match event {
                    Event::Key(k)
                        if k.kind == KeyEventKind::Press && k.code == KeyCode::Char('q') =>
                    {
                        break;
                    }
                    Event::Key(k)
                        if k.kind == KeyEventKind::Press && k.code == KeyCode::Char('o') =>
                    {
                        overlay = !overlay;
                        host_dirty = true;
                        if !overlay {
                            pane.invalidate();
                        }
                    }
                    Event::Resize(..) => {
                        host_dirty = true;
                        if let Some(font) = ratatui_slint::terminal::cell_size() {
                            pane.set_font_size(font);
                        }
                    }
                    _ if !overlay => {
                        router.route(&event, &[&pane]);
                    }
                    _ => {}
                },
                _ => {}
            }
            if runtime.begin_frame() {
                let area = Rect::from(session.terminal.size()?);
                pane.set_area(area);
                let pane_changed = pane.update();
                let host_changed = std::mem::take(&mut host_dirty);
                if let Some(e) = pane.last_error() {
                    return Err(std::io::Error::other(e).into());
                }
                if pane_changed || host_changed {
                    session.terminal.draw(|frame| {
                        frame.render_stateful_widget(SlintWidget, area, &mut pane);
                        if overlay {
                            let box_area =
                                Rect::new(area.x, area.y, area.width.min(35), area.height.min(3));
                            frame.render_widget(Clear, box_area);
                            frame.render_widget(
                                Paragraph::new("Overlay: press o to dismiss"),
                                box_area,
                            );
                        }
                    })?;
                }
            }
            runtime.write_pending_output(session.terminal.backend_mut())?;
        }
        Ok(())
    })();
    drop(pane);
    let cleanup = session.finish(&mut runtime).map_err(Into::into);
    result.and(cleanup)
}
