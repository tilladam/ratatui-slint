#![allow(dead_code)]
//! Shared fixtures for the headless tests. Every test thread installs its own Runtime: Slint's
//! platform slot is per thread.

use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::Rect;
use ratatui::{TerminalOptions, Viewport};
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_slint::output::{ByteCounter, CountingWriter};

slint::slint! {
    // Continuously animated: exercises per-frame repaint.
    export component Animated inherits Window {
        background: #1e2630;
        Rectangle {
            width: 20px; height: 20px; background: #4a90e2;
            x: (Math.mod(animation-tick() / 1ms, 2000) / 2000) * (root.width - self.width);
        }
    }

    // Static: nothing changes unless a property is set.
    export component Still inherits Window {
        in-out property <color> fill: #3bd16f;
        in-out property <length> logical-width: root.width;
        background: #202020;
        Rectangle { x: 10px; y: 10px; width: 40px; height: 20px; background: root.fill; }
    }

    // Keys: '+' and '-' accepted, everything else rejected.
    export component Gauge inherits Window {
        in-out property <float> value: 0.2;
        in-out property <int> clicks: 0;
        in-out property <int> presses: 0;
        in-out property <int> releases: 0;
        background: #1e2630;
        fs := FocusScope {
            key-pressed(e) => {
                root.presses += 1;
                if (e.text == "+") { root.value = Math.min(1.0, root.value + 0.1); return accept; }
                if (e.text == "-") { root.value = Math.max(0.0, root.value - 0.1); return accept; }
                reject
            }
            key-released(e) => { root.releases += 1; reject }
            TouchArea { clicked => { root.clicks += 1; } }
        }
        Rectangle {
            x: 0; y: root.height - 10px; height: 10px;
            width: root.width * root.value;
            background: #f5c542;
        }
        init => { fs.focus(); }
    }
}

/// Half-blocks at the 10 × 20 px fallback cell size: no terminal needed.
pub fn halfblocks() -> Picker {
    Picker::halfblocks()
}

pub fn picker(protocol: ProtocolType) -> Picker {
    #[allow(deprecated)]
    let mut p = Picker::from_fontsize(FontSize::new(10, 20));
    p.set_protocol_type(protocol);
    p
}

pub fn test_terminal(w: u16, h: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).unwrap()
}

/// A real crossterm backend writing into memory, with a byte counter. Fixed viewport, so it never
/// queries the (absent) terminal for its size.
pub fn counting_terminal(w: u16, h: u16) -> (Terminal<CrosstermBackend<CountingWriter<Vec<u8>>>>, ByteCounter) {
    let writer = CountingWriter::new(Vec::new());
    let counter = writer.counter();
    let term = Terminal::with_options(
        CrosstermBackend::new(writer),
        TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, w, h)) },
    )
    .unwrap();
    (term, counter)
}

/// Cells in `area` that the pane drew into: a glyph, or a coloured background (half-blocks
/// draws uniform cells as a space with a background colour).
pub fn painted_cells(buf: &ratatui::buffer::Buffer, area: Rect) -> usize {
    let mut n = 0;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &buf[(x, y)];
            if cell.symbol() != " " || cell.bg != ratatui::style::Color::Reset {
                n += 1;
            }
        }
    }
    n
}
