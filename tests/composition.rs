mod common;

use common::*;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    buffer::Buffer,
    layout::Rect,
    widgets::{Clear, Paragraph, StatefulWidget, Widget},
};
use ratatui_image::picker::ProtocolType;
use ratatui_slint::{EncodeMode, Runtime, SlintWidget};

fn draw_bytes(previous: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut backend = CrosstermBackend::new(&mut bytes);
    backend.draw(previous.diff(next).into_iter()).unwrap();
    backend.flush().unwrap();
    bytes
}

#[test]
fn kitty_upload_survives_overlay_over_first_cell() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt
        .create_pane(&picker(ProtocolType::Kitty), Still::new)
        .unwrap();
    pane.set_encode_mode(EncodeMode::Inline).unwrap();
    let area = Rect::new(0, 0, 8, 3);
    pane.set_area(area);
    pane.update();
    let empty = Buffer::empty(area);
    let mut covered = empty.clone();
    SlintWidget.render(area, &mut covered, &mut pane);
    let overlay = Rect::new(0, 0, 2, 1);
    Clear.render(overlay, &mut covered);
    Paragraph::new("hi").render(overlay, &mut covered);
    let mut bytes = draw_bytes(&empty, &covered);
    rt.write_pending_output(&mut bytes).unwrap();
    assert!(
        String::from_utf8_lossy(&bytes).contains("a=T"),
        "image upload lost when first cell was overwritten"
    );
}

#[test]
fn raw_invalidation_repaints_static_images_once_without_encoding() {
    let mut rt = Runtime::new().unwrap();
    for protocol in [ProtocolType::Sixel, ProtocolType::Iterm2] {
        let mut pane = rt.create_pane(&picker(protocol), Still::new).unwrap();
        pane.set_encode_mode(EncodeMode::Inline).unwrap();
        let screen = Rect::new(0, 0, 12, 5);
        let area = Rect::new(0, 0, 10, 3);
        pane.set_area(area);
        pane.update();
        let mut before = Buffer::empty(screen);
        SlintWidget.render(area, &mut before, &mut pane);
        let overlay = Rect::new(2, 1, 3, 1);
        Clear.render(overlay, &mut before);
        Paragraph::new("pop").render(overlay, &mut before);
        let frames = pane.frames();
        pane.invalidate();
        pane.invalidate(); // repeated damage before a draw must not toggle itself away
        assert!(pane.update());
        assert_eq!(pane.frames(), frames, "raw damage reencoded pixels");
        let mut after = Buffer::empty(screen);
        SlintWidget.render(area, &mut after, &mut pane);
        let bytes = draw_bytes(&before, &after);
        let bytes = String::from_utf8_lossy(&bytes);
        assert!(
            bytes.contains("\x1bP") || bytes.contains("\x1b]1337"),
            "{protocol:?} did not repaint"
        );
        assert!(!pane.update());
        let mut idle = Buffer::empty(screen);
        SlintWidget.render(area, &mut idle, &mut pane);
        assert!(after.diff(&idle).is_empty());
        assert!(matches!(
            rt.next_event_capped(Some(std::time::Duration::ZERO)),
            ratatui_slint::LoopEvent::Wake
        ));
    }
}

#[test]
fn kitty_replacement_upload_precedes_deletion_even_when_fully_occluded() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt
        .create_pane(&picker(ProtocolType::Kitty), Still::new)
        .unwrap();
    pane.set_encode_mode(EncodeMode::Inline).unwrap();
    let area = Rect::new(0, 0, 8, 3);
    pane.set_area(area);
    pane.update();
    let mut previous = Buffer::empty(area);
    SlintWidget.render(area, &mut previous, &mut pane);
    let mut first_upload = Vec::new();
    rt.write_pending_output(&mut first_upload).unwrap();
    pane.component()
        .set_fill(slint::Color::from_rgb_u8(200, 0, 0));
    pane.update();
    let mut covered = Buffer::empty(area);
    SlintWidget.render(area, &mut covered, &mut pane);
    Clear.render(area, &mut covered);
    let mut bytes = draw_bytes(&previous, &covered);
    rt.write_pending_output(&mut bytes).unwrap();
    let output = String::from_utf8_lossy(&bytes);
    assert!(output.find("a=T").unwrap() < output.find("a=d").unwrap());
    let mut uncovered = Buffer::empty(area);
    SlintWidget.render(area, &mut uncovered, &mut pane);
    let bytes = draw_bytes(&covered, &uncovered);
    assert!(String::from_utf8_lossy(&bytes).contains('\u{10eeee}'));
    let mut no_repeat = Vec::new();
    rt.write_pending_output(&mut no_repeat).unwrap();
    assert!(no_repeat.is_empty());
    pane.invalidate(); // storage loss: new ID and upload, even though properties did not change
    assert!(pane.update());
    SlintWidget.render(area, &mut uncovered, &mut pane);
    rt.write_pending_output(&mut no_repeat).unwrap();
    assert!(String::from_utf8_lossy(&no_repeat).contains("a=T"));
}
