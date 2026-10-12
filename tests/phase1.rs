//! Phase 1 exit criteria: two panes in one app, terminal resize, panes dropped without leaking
//! windows, one terminal frame per loop cycle, and a stable kitty image id.

mod common;

use std::time::Duration;

use common::*;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui_image::picker::ProtocolType;
use ratatui_slint::{EncodeMode, Runtime, SlintWidget};
use slint::ComponentHandle;

#[test]
fn two_panes_render_side_by_side() {
    let mut rt = Runtime::new().unwrap();
    let picker = halfblocks();
    let mut a = rt.create_pane(&picker, Still::new).unwrap();
    let mut b = rt.create_pane(&picker, Gauge::new).unwrap();
    assert_ne!(a.id(), b.id());
    assert_eq!(rt.live_windows(), 2);

    let mut term = test_terminal(60, 12);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50); 2]).areas(Rect::new(0, 0, 60, 12));
    a.set_area(left);
    b.set_area(right);
    assert!(rt.begin_frame());
    assert!(a.update());
    assert!(b.update());
    term.draw(|f| {
        f.render_stateful_widget(SlintWidget, left, &mut a);
        f.render_stateful_widget(SlintWidget, right, &mut b);
    })
    .unwrap();
    let buf = term.backend().buffer();
    assert_eq!(
        painted_cells(buf, left),
        (left.width * left.height) as usize
    );
    assert_eq!(
        painted_cells(buf, right),
        (right.width * right.height) as usize
    );
    assert_eq!((a.frames(), b.frames()), (1, 1));
}

#[test]
fn resize_reallocates_and_rerenders() {
    let mut rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    pane.set_scale(ratatui_slint::Scale::Fixed(1.0)); // logical width == pixel width below
    pane.set_area(Rect::new(0, 0, 20, 5));
    assert_eq!(pane.pixel_size(), Some((200, 100)));
    assert!(rt.begin_frame());
    assert!(pane.update());
    assert_eq!(pane.component().get_logical_width(), 200.0);

    // Terminal grew: new pixel size, Slint relayouts, a new frame is encoded.
    pane.set_area(Rect::new(0, 0, 30, 8));
    assert_eq!(pane.pixel_size(), Some((300, 160)));
    assert_eq!(pane.framebuffer().len(), 300 * 160);
    assert!(pane.update());
    assert_eq!(pane.frames(), 2);
    assert_eq!(pane.component().get_logical_width(), 300.0);

    // Moving without resizing redraws the terminal but does not re-render Slint.
    pane.set_area(Rect::new(5, 2, 30, 8));
    assert!(pane.update());
    assert_eq!(pane.frames(), 2);

    // Zero-sized and absurdly large areas hide the pane instead of allocating.
    pane.set_area(Rect::new(0, 0, 0, 8));
    assert_eq!(pane.pixel_size(), None);
    pane.set_area(Rect::new(0, 0, 1000, 1000));
    assert_eq!(pane.pixel_size(), None);
}

#[test]
fn dropped_panes_leave_no_windows() {
    let rt = Runtime::new().unwrap();
    let picker = halfblocks();
    let a = rt.create_pane(&picker, Still::new).unwrap();
    let b = rt.create_pane(&picker, Gauge::new).unwrap();
    let weak_a = a.component().as_weak();
    assert_eq!(rt.live_windows(), 2);
    drop(a);
    assert!(weak_a.upgrade().is_none(), "component outlived its pane");
    assert_eq!(rt.live_windows(), 1);
    drop(b);
    assert_eq!(rt.live_windows(), 0);
}

#[test]
fn factory_errors_are_reported() {
    let rt = Runtime::new().unwrap();
    let r = rt.create_pane(&halfblocks(), || Err::<Still, _>("no such component"));
    assert!(matches!(r, Err(ratatui_slint::Error::Component(_))));
    assert_eq!(rt.live_windows(), 0);
}

#[test]
fn second_runtime_on_one_thread_fails() {
    let _rt = Runtime::new().unwrap();
    assert!(matches!(
        Runtime::new(),
        Err(ratatui_slint::Error::Platform(_))
    ));
}

#[test]
fn idle_ui_blocks_and_writes_nothing() {
    let mut rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    let (mut term, bytes) = counting_terminal(30, 6);
    let area = Rect::new(0, 0, 30, 6);
    pane.set_area(area);
    // Process the initial layout wake and Slint's deferred window-setup timer before checking
    // steady-state idle behaviour. timeout() reports timers without firing them.
    assert!(matches!(rt.next_event(), ratatui_slint::LoopEvent::Wake));

    let mut draw = |rt: &mut Runtime, pane: &mut ratatui_slint::SlintPane<Still>| {
        if rt.begin_frame() && pane.update() {
            term.draw(|f| f.render_stateful_widget(SlintWidget, area, pane))
                .unwrap();
        }
    };
    draw(&mut rt, &mut pane);
    let after_first = bytes.get();
    assert!(after_first > 0);

    // Nothing animates and no timer runs: once the pending frame is consumed, the loop may block
    // indefinitely.
    std::thread::sleep(Duration::from_millis(40));
    draw(&mut rt, &mut pane);
    assert_eq!(rt.timeout(), None, "an idle UI must not wake the loop");
    for _ in 0..5 {
        let _ = rt.next_event_capped(Some(Duration::from_millis(40)));
        draw(&mut rt, &mut pane);
    }
    assert_eq!(
        bytes.get(),
        after_first,
        "idle frames wrote to the terminal"
    );

    // A property change produces exactly one more frame.
    pane.component()
        .set_fill(slint::Color::from_rgb_u8(200, 30, 30));
    rt.request_frame();
    std::thread::sleep(Duration::from_millis(40));
    draw(&mut rt, &mut pane);
    assert!(bytes.get() > after_first);
    assert_eq!(pane.frames(), 2);
}

#[test]
fn animation_keeps_the_loop_ticking_at_the_frame_cap() {
    let mut rt = Runtime::new().unwrap();
    rt.set_frame_rate(20);
    let mut pane = rt.create_pane(&halfblocks(), Animated::new).unwrap();
    pane.set_area(Rect::new(0, 0, 30, 6));
    assert!(rt.begin_frame());
    assert!(pane.update());
    assert!(rt.animating());
    // Frame just started: not due again until the 50 ms interval has passed.
    assert!(!rt.begin_frame());
    let t = rt.timeout().expect("animating UI needs a timeout");
    assert!(t <= Duration::from_millis(50), "{t:?}");

    let mut frames = 0;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        let _ = rt.next_event();
        if rt.begin_frame() && pane.update() {
            frames += 1;
        }
    }
    // 20 fps for 0.5 s is 10 frames; allow scheduler slack on both sides.
    assert!(
        (7..=11).contains(&frames),
        "rendered {frames} frames in 0.5 s at a 20 fps cap"
    );
}

#[test]
fn widget_alone_lays_out_the_pane() {
    let mut rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    let mut term = test_terminal(20, 5);
    let area = Rect::new(0, 0, 20, 5);
    let _ = rt.begin_frame();
    // No set_area: the first draw adopts the area, the next frame renders it.
    term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
        .unwrap();
    assert_eq!(pane.area(), area);
    assert!(matches!(
        rt.next_event_capped(Some(Duration::from_millis(100))),
        ratatui_slint::LoopEvent::Wake
    ));
    std::thread::sleep(Duration::from_millis(40));
    assert!(rt.begin_frame() && pane.update());
    term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
        .unwrap();
    assert_eq!(painted_cells(term.backend().buffer(), area), 100);
}

fn kitty_id_of_first_cell(term: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let first = term.backend().buffer()[(0, 0)].symbol().to_string();
    first
        .split("i=")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .map(str::to_owned)
        .expect("first cell carries the kitty transmit sequence")
}

/// `KittyIds::Stable`: every frame re-transmits under one image id.
#[test]
fn stable_kitty_ids_reuse_one_id() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt
        .create_pane(&picker(ProtocolType::Kitty), Still::new)
        .unwrap();
    pane.set_encode_mode(EncodeMode::Inline);
    pane.set_kitty_ids(ratatui_slint::KittyIds::Stable);
    let area = Rect::new(0, 0, 8, 2);
    pane.set_area(area);
    let mut term = test_terminal(8, 2);
    let mut ids = Vec::new();
    for color in [(200, 0, 0), (0, 200, 0), (0, 0, 200)] {
        pane.component()
            .set_fill(slint::Color::from_rgb_u8(color.0, color.1, color.2));
        assert!(pane.update());
        term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
            .unwrap();
        ids.push(kitty_id_of_first_cell(&term));
    }
    assert!(
        ids.windows(2).all(|w| w[0] == w[1]),
        "image ids changed between frames: {ids:?}"
    );
    let mut out = Vec::new();
    rt.write_pending_output(&mut out).unwrap();
    assert!(out.is_empty(), "stable ids never delete images mid-run");
}

/// `KittyIds::Fresh` (default): a new id per frame, and the replaced image is deleted only after
/// the frame that replaces it has been drawn.
#[test]
fn fresh_kitty_ids_delete_the_replaced_image_after_drawing() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt
        .create_pane(&picker(ProtocolType::Kitty), Still::new)
        .unwrap();
    pane.set_encode_mode(EncodeMode::Inline);
    pane.set_kitty_ids(ratatui_slint::KittyIds::Fresh);
    let area = Rect::new(0, 0, 8, 2);
    pane.set_area(area);
    let mut term = test_terminal(8, 2);

    assert!(pane.update());
    term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
        .unwrap();
    let first = kitty_id_of_first_cell(&term);
    let mut out = Vec::new();
    rt.write_pending_output(&mut out).unwrap();
    assert!(out.is_empty(), "nothing to delete after the first frame");

    pane.component()
        .set_fill(slint::Color::from_rgb_u8(0, 0, 200));
    assert!(pane.update());
    rt.write_pending_output(&mut out).unwrap();
    assert!(
        out.is_empty(),
        "the old image must stay until the new frame is drawn"
    );
    term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
        .unwrap();
    let second = kitty_id_of_first_cell(&term);
    assert_ne!(first, second);
    rt.write_pending_output(&mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out, format!("\x1b_Ga=d,d=I,i={first},q=2\x1b\\"));

    // Dropping the pane deletes the image on display.
    drop(pane);
    let mut out = Vec::new();
    rt.write_pending_output(&mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("\x1b_Ga=d,d=I,i={second},q=2\x1b\\")
    );
}

#[test]
fn waker_wakes_the_loop() {
    // Waker works for every runtime, including ones that do not own Slint's global proxy.
    let mut rt = Runtime::new().unwrap();
    let waker = rt.waker();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        waker.wake();
    });
    let _ = rt.begin_frame();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let ratatui_slint::LoopEvent::Wake =
            rt.next_event_capped(Some(Duration::from_millis(100)))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "waker never woke the loop"
        );
    }
}

#[test]
fn font_size_change_rerenders_at_the_new_cell_size() {
    use ratatui_slint::{InputRouter, PaneInput};
    let mut rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Gauge::new).unwrap();
    pane.set_area(Rect::new(0, 0, 20, 5));
    let _ = rt.begin_frame();
    assert!(pane.update());
    assert_eq!(pane.pixel_size(), Some((200, 100)));

    // Font zoomed out: same cells, fewer pixels per cell.
    pane.set_font_size(ratatui_image::FontSize::new(8, 16));
    assert_eq!(pane.pixel_size(), Some((160, 80)));
    assert_eq!(pane.framebuffer().len(), 160 * 80);
    assert!(pane.update());
    assert_eq!(pane.frames(), 2);
    assert_eq!(
        pane.displayed_cells(),
        Some(ratatui::layout::Size::new(20, 5))
    );

    // Pointer input still lands in the pane after the change.
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    let at = |kind| {
        crossterm::event::Event::Mouse(crossterm::event::MouseEvent {
            kind,
            column: 19,
            row: 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    };
    use crossterm::event::{MouseButton, MouseEventKind};
    router.route(&at(MouseEventKind::Down(MouseButton::Left)), &panes);
    router.route(&at(MouseEventKind::Up(MouseButton::Left)), &panes);
    assert_eq!(pane.component().get_clicks(), 1);
}

#[test]
fn overdue_one_shot_timer_wakes_and_renders_a_static_pane() {
    let mut rt = Runtime::new().unwrap();
    rt.set_frame_rate(1000);
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    pane.set_area(Rect::new(0, 0, 10, 3));
    assert!(matches!(rt.next_event(), ratatui_slint::LoopEvent::Wake));
    assert!(rt.begin_frame());
    assert!(pane.update());
    assert_eq!(rt.timeout(), None);

    let weak = pane.component().as_weak();
    let color = slint::Color::from_rgb_u8(255, 0, 0);
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        weak.upgrade().unwrap().set_fill(color);
    });
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rt.timeout(), Some(Duration::ZERO));
    assert!(matches!(
        rt.next_event_capped(Some(Duration::from_millis(100))),
        ratatui_slint::LoopEvent::Timeout
    ));
    assert_eq!(pane.component().get_fill(), color);
    assert!(rt.begin_frame());
    assert!(pane.update());
    assert_eq!(pane.frames(), 2);
    assert_eq!(rt.timeout(), None, "completed timer must not busy-loop");
}

#[test]
fn moving_an_idle_pane_wakes_without_reencoding() {
    let mut rt = Runtime::new().unwrap();
    rt.set_frame_rate(1000);
    let mut pane = rt.create_pane(&halfblocks(), Still::new).unwrap();
    pane.set_area(Rect::new(0, 0, 10, 3));
    assert!(matches!(rt.next_event(), ratatui_slint::LoopEvent::Wake));
    assert!(rt.begin_frame());
    assert!(pane.update());
    assert_eq!(rt.timeout(), None);
    let moved = Rect::new(2, 1, 10, 3);
    pane.set_area(moved);
    assert!(matches!(
        rt.next_event_capped(Some(Duration::from_millis(100))),
        ratatui_slint::LoopEvent::Wake
    ));
    std::thread::sleep(Duration::from_millis(2));
    assert!(rt.begin_frame());
    assert!(pane.update());
    assert_eq!(pane.frames(), 1);
    let mut term = test_terminal(14, 5);
    term.draw(|f| f.render_stateful_widget(SlintWidget, moved, &mut pane))
        .unwrap();
    assert_eq!(painted_cells(term.backend().buffer(), moved), 30);
}

#[test]
fn dropping_before_a_replacement_draw_deletes_retired_kitty_images() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt
        .create_pane(&picker(ProtocolType::Kitty), Still::new)
        .unwrap();
    pane.set_encode_mode(EncodeMode::Inline);
    pane.set_kitty_ids(ratatui_slint::KittyIds::Fresh);
    let area = Rect::new(0, 0, 8, 2);
    pane.set_area(area);
    assert!(pane.update());
    let mut term = test_terminal(8, 2);
    term.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))
        .unwrap();
    let first = kitty_id_of_first_cell(&term);
    pane.component()
        .set_fill(slint::Color::from_rgb_u8(255, 0, 0));
    assert!(pane.update());
    pane.component()
        .set_fill(slint::Color::from_rgb_u8(0, 0, 255));
    assert!(pane.update());
    let mut out = Vec::new();
    rt.write_pending_output(&mut out).unwrap();
    assert!(out.is_empty(), "old image must survive until draw or drop");
    drop(pane);
    rt.write_pending_output(&mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains(&format!("\x1b_Ga=d,d=I,i={first},q=2\x1b\\")));
    assert_eq!(
        out.matches("a=d,d=I").count(),
        3,
        "all accepted IDs must be deleted"
    );
}
