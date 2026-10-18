//! Frame rate of a continuously animated pane in the terminal it runs in.
//!
//! `cargo run --release --example termbench [seconds]`
//!
//! Runs a full-screen animated pane for a few seconds (default 5) at a 60 fps cap, restores the
//! terminal and prints where the time went. "write" includes the frame and pending-output
//! flush: when the terminal cannot consume the output as fast as it is produced, the pty
//! fills up and this grows. Compare terminals, and runs with `RATATUI_SLINT_KITTY_COMPRESSION=1`
//! versus `RATATUI_SLINT_KITTY_COMPRESSION=0`.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
use ratatui_slint::{LoopEvent, Runtime, SlintWidget};

slint::slint! {
    export component Bench inherits Window {
        background: #10151b;
        Rectangle {
            width: 20%; height: 30%; y: root.height * 0.1; border-radius: 12px;
            background: @linear-gradient(90deg, #1f6feb 0%, #3fb950 100%);
            x: (Math.mod(animation-tick() / 1ms, 2000) / 2000) * (root.width - self.width);
        }
        Text {
            x: root.width * 0.05; y: root.height * 0.6;
            text: "termbench " + Math.round(Math.mod(animation-tick() / 1ms, 100000) / 100) / 10;
            color: #e6edf3; font-size: 32px;
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let mut rt = Runtime::new()?;
    rt.set_frame_rate(60);
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    rt.spawn_terminal_reader()?;
    let mut pane = rt.create_pane(&session.picker, Bench::new)?;

    let (mut frames, mut render, mut snapshot, mut encode, mut write) = (
        0u32,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
    );
    let mut max_write = Duration::ZERO;
    let start_bytes = session.bytes.get();
    let started = Instant::now();
    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        while started.elapsed() < Duration::from_secs(seconds) {
            match rt.next_event_capped(Some(Duration::from_millis(100))) {
                LoopEvent::Quit => break,
                LoopEvent::InputError(error) => return Err(error.into()),
                _ => {}
            }
            if !rt.begin_frame() {
                continue;
            }
            let area = Rect::from(session.terminal.size()?);
            pane.set_area(area);
            let changed = pane.update();
            if let Some(error) = pane.last_error() {
                return Err(std::io::Error::other(error).into());
            }
            if changed {
                let s = pane.stats();
                let t = Instant::now();
                session
                    .terminal
                    .draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))?;
                rt.write_pending_output(session.terminal.backend_mut())?;
                let w = t.elapsed();
                frames += 1;
                render += s.render;
                snapshot += s.snapshot;
                encode += s.encode;
                write += w;
                max_write = max_write.max(w);
            }
        }
        Ok(())
    })();
    let elapsed = started.elapsed().as_secs_f64();
    let bytes = session.bytes.get() - start_bytes;
    let protocol = pane.protocol_type();
    let font = pane.font_size();
    let (px, cells, scale) = (pane.pixel_size(), pane.area(), pane.scale_factor());
    drop(pane);
    let cleanup = session.finish(&mut rt).map_err(Into::into);
    result.and(cleanup)?;

    let n = f64::from(frames.max(1));
    let ms = |d: Duration| d.as_secs_f64() * 1e3 / n;
    println!(
        "terminal   {}",
        std::env::var("TERM_PROGRAM").unwrap_or_else(|_| std::env::var("TERM").unwrap_or_default())
    );
    println!(
        "protocol   {protocol:?}, cell {}x{} px, scale {scale:.2}",
        font.width, font.height
    );
    println!(
        "pane       {}x{} cells, {:?} px",
        cells.width, cells.height, px
    );
    println!(
        "frames     {frames} in {elapsed:.1} s = {:.1} fps (cap 60)",
        f64::from(frames) / elapsed
    );
    println!(
        "per frame  render {:.2} ms, snapshot {:.2} ms, encode {:.2} ms, write {:.2} ms (max {:.1} ms)",
        ms(render),
        ms(snapshot),
        ms(encode),
        ms(write),
        max_write.as_secs_f64() * 1e3
    );
    println!(
        "output     {:.2} MB/frame, {:.1} MB/s",
        bytes as f64 / n / 1e6,
        bytes as f64 / elapsed / 1e6
    );
    Ok(())
}
