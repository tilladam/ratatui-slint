mod common;
use common::*;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui_slint::{InputRouter, PaneInput, Runtime};

fn mouse(kind: MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers,
    })
}

slint::slint! {
    export component Modifiers inherits Window {
        in-out property <bool> control-seen;
        in-out property <bool> shift-seen;
        in-out property <int> key-count;
        in-out property <int> clicks;
        in-out property <int> cancels;
        in-out property <int> ups;
        in-out property <bool> wheel-shift;

        fs := FocusScope {
            key-pressed(e) => { root.key-count += 1; root.control-seen = e.modifiers.control; accept }
            TouchArea {
                pointer-event(e) => {
                    root.shift-seen = e.modifiers.shift;
                    if (e.kind == PointerEventKind.cancel) { root.cancels += 1; }
                    if (e.kind == PointerEventKind.up) { root.ups += 1; }
                }
                clicked => { root.clicks += 1; }
                scroll-event(e) => { root.wheel-shift = e.modifiers.shift; accept }
            }
        }
        init => { fs.focus(); }
    }
}

#[test]
fn focus_loss_cancels_capture() {
    let rt = Runtime::new().unwrap();
    let mut a = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    let mut b = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 5));
    b.set_area(Rect::new(10, 0, 10, 5));
    let panes: [&dyn PaneInput; 2] = [&a, &b];
    let mut router = InputRouter::new();
    router.route(
        &mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            2,
            KeyModifiers::NONE,
        ),
        &panes,
    );
    router.route(&Event::FocusLost, &panes);
    assert_eq!(a.component().get_cancels(), 1);
    assert_eq!(a.component().get_clicks(), 0);
    router.route(&Event::FocusGained, &panes);
    router.route(
        &mouse(
            MouseEventKind::Up(MouseButton::Left),
            12,
            2,
            KeyModifiers::NONE,
        ),
        &panes,
    );
    assert_eq!(
        b.component().get_ups(),
        0,
        "release of canceled press reached another pane"
    );
    router.route(
        &mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            2,
            KeyModifiers::NONE,
        ),
        &panes,
    );
    assert_eq!(
        router.focused(),
        Some(b.id()),
        "new click was captured by old pane"
    );
}

#[test]
fn shift_click_delivers_modifiers() {
    let rt = Runtime::new().unwrap();
    let mut p = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    p.set_area(Rect::new(0, 0, 20, 5));
    let mut router = InputRouter::new();
    router.route(
        &mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            2,
            KeyModifiers::SHIFT,
        ),
        &[&p],
    );
    assert!(
        p.component().get_shift_seen(),
        "mouse modifier was discarded"
    );
}

#[test]
fn removed_pane_does_not_leave_modifiers_held() {
    let rt = Runtime::new().unwrap();
    let mut a = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    let mut b = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 5));
    b.set_area(Rect::new(10, 0, 10, 5));
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(true);
    router.focus(a.id(), &[&a, &b]);
    router.route(
        &Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
        &[&a, &b],
    );
    drop(a);
    router.route(&Event::Resize(20, 5), &[&b]);
    router.focus(b.id(), &[&b]);
    router.route(
        &Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
        &[&b],
    );
    assert!(
        b.component().get_control_seen(),
        "modifier state was not synchronized after removal"
    );
}

slint::slint! {
    export component PopupPane inherits Window {
        in-out property <int> base-hits;
        in-out property <int> popup-hits;
        TouchArea { clicked => { root.base-hits += 1; } }
        pop := PopupWindow {
            x: 0; y: 0; width: 80px; height: 60px;
            close-policy: close-on-click-outside;
            TouchArea { clicked => { root.popup-hits += 1; } }
        }
        public function open-popup() { pop.show(); }
    }
}

#[test]
fn clicking_other_pane_dismisses_popup() {
    let rt = Runtime::new().unwrap();
    let mut a = rt.create_pane(&halfblocks(), PopupPane::new).unwrap();
    let mut b = rt.create_pane(&halfblocks(), Still::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 5));
    b.set_area(Rect::new(10, 0, 10, 5));
    let panes: [&dyn PaneInput; 2] = [&a, &b];
    let mut router = InputRouter::new();
    router.focus(a.id(), &panes);
    a.component().invoke_open_popup();
    for x in [12, 2] {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            router.route(&mouse(kind, x, 1, KeyModifiers::NONE), &panes);
        }
    }
    assert_eq!(
        a.component().get_popup_hits(),
        0,
        "popup remained open when clicking another pane"
    );
    assert_eq!(a.component().get_base_hits(), 1);
}

#[test]
fn wheel_modifiers_use_keyboard_focused_window() {
    let rt = Runtime::new().unwrap();
    let mut a = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    let mut b = rt.create_pane(&halfblocks(), Modifiers::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 5));
    b.set_area(Rect::new(10, 0, 10, 5));
    let mut router = InputRouter::new();
    router.focus(a.id(), &[&a, &b]);
    router.route(
        &mouse(MouseEventKind::ScrollDown, 12, 2, KeyModifiers::SHIFT),
        &[&a, &b],
    );
    assert!(b.component().get_wheel_shift());
    assert_eq!(b.component().get_key_count(), 0);
    assert_eq!(router.focused(), Some(a.id()));
    assert!(a.component().get_key_count() > 0);
    router.blur(&[&a, &b]);
    router.route(
        &mouse(MouseEventKind::ScrollDown, 12, 2, KeyModifiers::SHIFT),
        &[&a, &b],
    );
    assert!(b.component().get_wheel_shift());
    assert_eq!(router.focused(), None);
}

#[test]
fn overlapping_panes_do_not_both_receive_the_transfer_click() {
    let rt = Runtime::new().unwrap();
    let mut a = rt.create_pane(&halfblocks(), PopupPane::new).unwrap();
    let mut b = rt.create_pane(&halfblocks(), PopupPane::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 5));
    b.set_area(a.area());
    let mut router = InputRouter::new();
    router.focus(a.id(), &[&b, &a]);
    a.component().invoke_open_popup();
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        router.route(&mouse(kind, 2, 1, KeyModifiers::NONE), &[&b, &a]);
    }
    assert_eq!(a.component().get_base_hits(), 0);
    assert_eq!(a.component().get_popup_hits(), 0);
    assert_eq!(b.component().get_base_hits(), 1);
    // Removing B makes A hittable. Its popup must already be dismissed.
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        router.route(&mouse(kind, 2, 1, KeyModifiers::NONE), &[&a]);
    }
    assert_eq!(a.component().get_base_hits(), 1);
    assert_eq!(a.component().get_popup_hits(), 0);
}
