//! Snapshot of a text-free component rendered through half-blocks on ratatui's TestBackend.
//!
//! Text is left out on purpose: system fonts differ between machines. Regenerate with
//! `UPDATE_SNAPSHOTS=1 cargo test --test snapshot`.

mod common;

use common::*;
use ratatui::layout::Rect;
use ratatui_slint::{Runtime, SlintWidget};

fn render_cells(buf: &ratatui::buffer::Buffer, area: Rect) -> String {
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let c = &buf[(x, y)];
            out.push_str(&format!("{}{:?}/{:?} ", c.symbol(), c.fg, c.bg));
        }
        out.push('\n');
    }
    out
}

#[test]
fn still_component_halfblocks_snapshot() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    pane.set_scale(ratatui_slint::Scale::Fixed(1.0)); // the snapshot covers rendering, not scaling
    let area = Rect::new(0, 0, 8, 3);
    pane.set_area(area);
    assert!(pane.update());
    let mut term = test_terminal(8, 3);
    term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
        .unwrap();
    let actual = render_cells(term.backend().buffer(), area);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/still_halfblocks.txt");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, &actual).unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        actual, expected,
        "half-block output changed; rerun with UPDATE_SNAPSHOTS=1 if intended"
    );
}
