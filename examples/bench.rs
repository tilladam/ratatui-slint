//! Per-protocol cost of an animated pane, measured at the terminal writer.
//!
//! `cargo run --release --example bench`
//!
//! Unlike the spike's bench, the byte column counts what the crossterm backend actually writes
//! (ratatui's diff included), not the size of the widget's cell symbols. No terminal is needed:
//! the backend writes into memory. Terminal decode time is not measured.

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{TerminalOptions, Viewport};
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_slint::output::CountingWriter;
use ratatui_slint::{EncodeMode, Runtime, SlintWidget};

slint::slint! {
    export component Bench inherits Window {
        in property <bool> full-redraw;
        background: #1e2630;
        // Small moving square: a typical dirty region.
        Rectangle {
            width: 24px; height: 24px; y: 12px; border-radius: 6px; background: #4a90e2;
            x: (Math.mod(animation-tick() / 1ms, 2000) / 2000) * (root.width - self.width);
        }
        // Optional full-pane change every frame: the worst case.
        if root.full-redraw: Rectangle {
            background: Math.mod(animation-tick() / 16ms, 2) == 0 ? #303840 : #283038;
            opacity: 0.5;
        }
    }
}

const COLS: u16 = 120;
const ROWS: u16 = 30;
const FRAMES: u32 = 30;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rt = Runtime::new()?;
    rt.set_frame_rate(1000);
    let area = Rect::new(0, 0, COLS, ROWS);
    println!(
        "pane {COLS} x {ROWS} cells = {} x {} px, {FRAMES} frames each",
        COLS * 10,
        ROWS * 20
    );
    println!(
        "{:<11} {:<6} {:>10} {:>12} {:>10} {:>12} {:>14}",
        "protocol", "change", "render ms", "snapshot ms", "encode ms", "draw ms", "bytes/frame"
    );
    for full in [false, true] {
        for protocol in [
            ProtocolType::Halfblocks,
            ProtocolType::Sixel,
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
        ] {
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize(FontSize::new(10, 20));
            picker.set_protocol_type(protocol);
            let mut pane = rt.create_pane(&picker, Bench::new)?;
            pane.component().set_full_redraw(full);
            pane.set_encode_mode(EncodeMode::Inline)?;
            pane.set_area(area);

            let writer = CountingWriter::new(std::io::sink());
            let bytes = writer.counter();
            let mut term = Terminal::with_options(
                CrosstermBackend::new(writer),
                TerminalOptions {
                    viewport: Viewport::Fixed(area),
                },
            )?;
            // Warm-up frame: first transmit and full screen paint are not steady state.
            pane.update();
            term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))?;
            let start_bytes = bytes.get();

            let (mut render, mut snapshot, mut encode, mut draw) = (
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
            );
            for _ in 0..FRAMES {
                std::thread::sleep(Duration::from_millis(17));
                slint::platform::update_timers_and_animations();
                pane.update();
                let s = pane.stats();
                render += s.render;
                snapshot += s.snapshot;
                encode += s.encode;
                let t = Instant::now();
                term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))?;
                draw += t.elapsed();
            }
            let per_frame = (bytes.get() - start_bytes) / u64::from(FRAMES);
            let ms = |d: Duration| d.as_secs_f64() * 1e3 / f64::from(FRAMES);
            println!(
                "{:<11} {:<6} {:>10.2} {:>12.2} {:>10.2} {:>12.2} {:>14}",
                format!("{protocol:?}"),
                if full { "full" } else { "small" },
                ms(render),
                ms(snapshot),
                ms(encode),
                ms(draw),
                per_frame
            );
        }
    }
    Ok(())
}
