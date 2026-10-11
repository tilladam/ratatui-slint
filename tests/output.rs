//! Phase 3: encoding off the UI thread, stale-frame rejection, scale policy, every protocol.

mod common;

use std::time::{Duration, Instant};

use common::*;
use ratatui::layout::Rect;
use ratatui_image::picker::ProtocolType;
use ratatui_slint::{EncodeMode, LoopEvent, Runtime, Scale, SlintWidget};

/// Runs the loop until the pane has encoded one more frame than it had on entry.
fn wait_for_frame<C: slint::ComponentHandle>(rt: &mut Runtime, pane: &mut ratatui_slint::SlintPane<C>) -> bool {
    let target = pane.frames() + 1;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let _ = rt.next_event_capped(Some(Duration::from_millis(50)));
        pane.update();
        if pane.frames() >= target {
            return true;
        }
    }
    false
}

#[test]
fn threaded_encoder_wakes_the_loop_when_done() {
    let mut rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&picker(ProtocolType::Sixel), Still::new).unwrap();
    pane.set_encode_mode(EncodeMode::Threaded);
    pane.set_area(Rect::new(0, 0, 20, 4));
    let _ = rt.begin_frame();
    // The render is submitted; the encoded frame arrives later, through a wake-up.
    pane.update();
    if pane.frames() == 0 {
        assert!(matches!(rt.next_event_capped(Some(Duration::from_secs(5))), LoopEvent::Wake));
        assert!(wait_for_frame(&mut rt, &mut pane), "encoder never delivered");
    }
    assert_eq!(pane.frames(), 1);
    assert_eq!(pane.last_error(), None);
}

#[test]
fn frames_encoded_for_an_old_size_are_dropped() {
    let mut rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&picker(ProtocolType::Sixel), Still::new).unwrap();
    pane.set_encode_mode(EncodeMode::Threaded);
    pane.set_area(Rect::new(0, 0, 60, 10));
    pane.update(); // submits a 600 × 200 px frame
    pane.set_area(Rect::new(0, 0, 30, 5)); // resize before it finishes
    assert!(wait_for_frame(&mut rt, &mut pane));
    // Only the frame for the new size was accepted.
    assert_eq!(pane.frames(), 1);
    assert_eq!(pane.displayed_cells(), Some(ratatui::layout::Size::new(30, 5)));
    let mut term = test_terminal(60, 10);
    term.draw(|f| f.render_stateful_widget(SlintWidget, Rect::new(0, 0, 30, 5), &mut pane)).unwrap();
}

#[test]
fn fit_scale_maps_a_fixed_design_onto_the_pane() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    pane.set_scale(Scale::Fit { width: 400.0, height: 200.0 });
    pane.set_area(Rect::new(0, 0, 80, 20)); // 800 × 400 px
    assert_eq!(pane.scale_factor(), 2.0);
    assert_eq!(pane.component().get_logical_width(), 400.0);
    // Wider than the design's aspect: height limits the factor.
    pane.set_area(Rect::new(0, 0, 120, 20)); // 1200 × 400 px
    assert_eq!(pane.scale_factor(), 2.0);
    assert_eq!(pane.component().get_logical_width(), 600.0);
    pane.set_scale(Scale::Fixed(1.0));
    assert_eq!(pane.component().get_logical_width(), 1200.0);
}

#[test]
fn every_protocol_encodes() {
    let mut rt = Runtime::new().unwrap();
    for protocol in [ProtocolType::Halfblocks, ProtocolType::Sixel, ProtocolType::Kitty, ProtocolType::Iterm2] {
        let mut pane = rt.create_pane(&picker(protocol), Still::new).unwrap();
        pane.set_encode_mode(EncodeMode::Inline);
        let area = Rect::new(0, 0, 12, 3);
        pane.set_area(area);
        let _ = rt.begin_frame();
        assert!(pane.update(), "{protocol:?}");
        assert_eq!(pane.last_error(), None, "{protocol:?}");
        let mut term = test_terminal(12, 3);
        term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane)).unwrap();
        assert!(painted_cells(term.backend().buffer(), area) > 0, "{protocol:?} drew nothing");
    }
}

#[test]
fn encode_mode_auto_threads_slow_protocols_only() {
    let mut rt = Runtime::new().unwrap();
    // Half-blocks: inline, so the first update already has the frame.
    let mut hb = rt.create_pane(&halfblocks(), Still::new).unwrap();
    hb.set_area(Rect::new(0, 0, 10, 3));
    let _ = rt.begin_frame();
    assert!(hb.update());
    // Sixel: threaded, delivered through a wake-up.
    let mut sx = rt.create_pane(&picker(ProtocolType::Sixel), Still::new).unwrap();
    sx.set_area(Rect::new(0, 0, 10, 3));
    sx.update();
    assert_eq!(sx.frames(), 0, "sixel should not encode on the UI thread");
    assert!(wait_for_frame(&mut rt, &mut sx));
}

#[cfg(feature = "transparency")]
mod transparency {
    use super::*;

    slint::slint! {
        export component Clear inherits Window {
            background: transparent;
            Rectangle { x: 0; y: 0; width: 10px; height: 20px; background: #ff0000; }
        }
    }

    #[test]
    fn transparent_pixels_are_composited_for_halfblocks() {
        let rt = Runtime::new().unwrap();
        let mut pane = rt.create_pane(&halfblocks(), Clear::new).unwrap();
        pane.set_background([10, 20, 30]);
        let area = Rect::new(0, 0, 4, 2);
        pane.set_area(area);
        assert!(pane.update());
        assert_eq!(pane.framebuffer()[pane.framebuffer().len() - 1].alpha, 0);
        let mut term = test_terminal(4, 2);
        term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane)).unwrap();
        let buf = term.backend().buffer();
        let far = &buf[(3, 1)];
        assert!(
            far.bg == ratatui::style::Color::Rgb(10, 20, 30) || far.fg == ratatui::style::Color::Rgb(10, 20, 30),
            "{far:?}"
        );
    }
}
