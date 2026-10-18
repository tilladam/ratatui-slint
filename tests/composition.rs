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
    let mut previous = Buffer::empty(area);
    let mut stream = Vec::new();
    // One host-loop step: update, draw (optionally fully covered), then write pending output.
    let mut step = |pane: &mut ratatui_slint::SlintPane<Still>, covered: bool| {
        pane.update();
        let mut next = Buffer::empty(area);
        SlintWidget.render(area, &mut next, pane);
        if covered {
            Clear.render(area, &mut next);
        }
        let mut bytes = draw_bytes(&previous, &next);
        rt.write_pending_output(&mut bytes).unwrap();
        previous = next;
        stream.extend_from_slice(&bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    };
    // A Kitty frame is uploaded by one step and drawn by the next.
    step(&mut pane, false);
    step(&mut pane, false);
    pane.component()
        .set_fill(slint::Color::from_rgb_u8(200, 0, 0));
    let output = step(&mut pane, true) + &step(&mut pane, true);
    assert!(output.find("a=T").unwrap() < output.find("a=d").unwrap());
    let output = step(&mut pane, false);
    assert!(output.contains('\u{10eeee}'));
    assert!(!output.contains("a=T"), "unchanged image uploaded again");
    pane.invalidate(); // storage loss: new ID and upload, even though properties did not change
    assert!(step(&mut pane, false).contains("a=T"));
    step(&mut pane, false);
    placeholders_follow_uploads(&stream).unwrap();
}

/// Kitty draws nothing for a placeholder whose image has not arrived, so every placeholder must
/// follow its image's upload in the byte stream. Checks the low 24 bits of the image ID, which
/// the placeholder carries as its foreground colour.
fn placeholders_follow_uploads(stream: &[u8]) -> Result<usize, String> {
    let text = String::from_utf8_lossy(stream);
    let mut uploaded = std::collections::HashSet::new();
    let mut fg = None;
    let mut checked = 0;
    let mut rest = &text[..];
    while let Some(c) = rest.chars().next() {
        if let Some(apc) = rest.strip_prefix("\x1b_G") {
            let end = apc.find("\x1b\\").ok_or("unterminated graphics command")?;
            let keys = apc[..end].split(';').next().unwrap_or("");
            if keys.split(',').any(|kv| kv == "a=T") {
                let id = keys
                    .split(',')
                    .find_map(|kv| kv.strip_prefix("i="))
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or("upload without an image ID")?;
                uploaded.insert(id & 0xff_ffff);
            }
            rest = &apc[end + 2..];
            continue;
        }
        if let Some(csi) = rest.strip_prefix("\x1b[") {
            let end = csi
                .find(|ch: char| ch.is_ascii_alphabetic())
                .ok_or("unterminated CSI")?;
            if csi[end..].starts_with('m') {
                let params: Vec<&str> = csi[..end].split(';').collect();
                if let Some(i) = params.windows(2).position(|w| w == ["38", "2"])
                    && let [r, g, b] = params[i + 2..i + 5]
                {
                    let rgb = |s: &str| s.parse::<u32>().unwrap_or(0);
                    fg = Some(rgb(r) << 16 | rgb(g) << 8 | rgb(b));
                }
            }
            rest = &csi[end + 1..];
            continue;
        }
        if c == '\u{10eeee}' {
            let id = fg.ok_or("placeholder without an image colour")?;
            if !uploaded.contains(&id) {
                return Err(format!("placeholder for image {id:#08x} before its upload"));
            }
            checked += 1;
        }
        rest = &rest[c.len_utf8()..];
    }
    Ok(checked)
}

#[test]
fn kitty_placeholders_never_precede_their_upload() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt
        .create_pane(&picker(ProtocolType::Kitty), Still::new)
        .unwrap();
    pane.set_encode_mode(EncodeMode::Inline).unwrap();
    let area = Rect::new(0, 0, 8, 3);
    pane.set_area(area);
    // A host loop: update, draw, then write pending output, as the integration guide requires.
    let mut previous = Buffer::empty(area);
    let mut stream = Vec::new();
    for step in 0..6u8 {
        if step % 2 == 1 {
            // Each new frame gets a fresh image ID, as an animation would.
            pane.component()
                .set_fill(slint::Color::from_rgb_u8(40 * step, 0, 0));
        }
        pane.update();
        let mut next = Buffer::empty(area);
        SlintWidget.render(area, &mut next, &mut pane);
        stream.extend(draw_bytes(&previous, &next));
        rt.write_pending_output(&mut stream).unwrap();
        previous = next;
    }
    let checked = placeholders_follow_uploads(&stream).unwrap();
    assert!(checked > 0, "no placeholders were drawn");
}
