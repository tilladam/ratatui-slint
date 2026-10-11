//! Phase 2 exit criteria: LineEdit, TextEdit and a ComboBox popup usable inside a pane;
//! unconsumed keys reach ratatui; a drag that leaves the pane still delivers its release there;
//! a popup closes on a click outside the pane.

mod common;

use common::*;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui_slint::{InputRouter, PaneInput, Routed, Runtime};

slint::slint! {
    import { LineEdit, TextEdit, ComboBox } from "std-widgets.slint";

    // 400 × 300 px at 10 × 20 px cells: 40 × 15 cells.
    export component Editor inherits Window {
        in-out property <string> line <=> le.text;
        in-out property <string> body <=> te.text;
        in-out property <string> choice <=> cb.current-value;
        in-out property <int> popup-hits;
        in-out property <int> under-hits;
        in-out property <length> drag-x;
        in-out property <int> ups;
        background: #222;
        le := LineEdit { x: 10px; y: 10px; width: 200px; height: 30px; }
        te := TextEdit { x: 10px; y: 50px; width: 200px; height: 80px; }
        cb := ComboBox { x: 10px; y: 140px; width: 150px; height: 30px; model: ["one", "two", "three"]; }
        TouchArea {
            x: 250px; y: 10px; width: 100px; height: 100px;
            pointer-event(e) => { if (e.kind == PointerEventKind.up) { root.ups += 1; } }
            moved => { root.drag-x = self.mouse-x; }
        }
        TouchArea {
            x: 250px; y: 150px; width: 100px; height: 100px;
            clicked => { root.under-hits += 1; }
        }
        popup := PopupWindow {
            x: 250px; y: 150px; width: 100px; height: 100px;
            close-policy: close-on-click-outside;
            Rectangle { background: #844; }
            TouchArea { clicked => { root.popup-hits += 1; } }
        }
        public function open-popup() { popup.show(); }
    }
}

const AREA: Rect = Rect::new(5, 2, 40, 15);

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn key_kind(code: KeyCode, kind: KeyEventKind) -> Event {
    Event::Key(KeyEvent { code, modifiers: KeyModifiers::NONE, kind, state: KeyEventState::NONE })
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
}

/// Cell for a logical pixel position inside the pane (cells are 10 × 20 px).
fn cell(x: u16, y: u16) -> (u16, u16) {
    (AREA.x + x / 10, AREA.y + y / 20)
}

fn click(router: &mut InputRouter, panes: &[&dyn PaneInput], x: u16, y: u16) -> Routed {
    let (c, r) = cell(x, y);
    let down = router.route(&mouse(MouseEventKind::Down(MouseButton::Left), c, r), panes);
    router.route(&mouse(MouseEventKind::Up(MouseButton::Left), c, r), panes);
    down
}

fn type_text(router: &mut InputRouter, panes: &[&dyn PaneInput], text: &str) {
    for ch in text.chars() {
        let mods = if ch.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        assert_eq!(router.route(&key(KeyCode::Char(ch), mods), panes), Routed::Consumed, "typing {ch:?}");
    }
}

fn editor() -> (Runtime, ratatui_slint::SlintPane<Editor>) {
    let rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Editor::new).unwrap();
    // The test geometry is in terminal pixels, so pin one logical pixel to one terminal pixel.
    pane.set_scale(ratatui_slint::Scale::Fixed(1.0));
    pane.set_area(AREA);
    (rt, pane)
}

#[test]
fn line_edit_takes_typing_and_shortcuts() {
    let (rt, pane) = editor();
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];

    assert_eq!(click(&mut router, &panes, 60, 25), Routed::Consumed);
    assert_eq!(router.focused(), Some(pane.id()));
    type_text(&mut router, &panes, "Hello");
    assert_eq!(pane.component().get_line(), "Hello");

    // Ctrl+A selects all, typing replaces the selection: modifiers reach Slint.
    router.route(&key(KeyCode::Char('a'), KeyModifiers::CONTROL), &panes);
    type_text(&mut router, &panes, "abc");
    assert_eq!(pane.component().get_line(), "abc");
    router.route(&key(KeyCode::Backspace, KeyModifiers::NONE), &panes);
    assert_eq!(pane.component().get_line(), "ab");

    // Copy goes to the cache and out as OSC 52.
    router.route(&key(KeyCode::Char('a'), KeyModifiers::CONTROL), &panes);
    router.route(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), &panes);
    assert_eq!(rt.clipboard_text().as_deref(), Some("ab"));
    let mut out = Vec::new();
    rt.write_pending_output(&mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("]52;c;YWI="));
    let mut again = Vec::new();
    rt.write_pending_output(&mut again).unwrap();
    assert!(again.is_empty(), "OSC 52 output must be written once");
}

#[test]
fn bracketed_paste_inserts_into_focused_field() {
    let (_rt, pane) = editor();
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    click(&mut router, &panes, 60, 25);
    assert_eq!(router.route(&Event::Paste("pasted text".into()), &panes), Routed::Consumed);
    assert_eq!(pane.component().get_line(), "pasted text");
}

#[test]
fn text_edit_takes_multiline_input_and_scroll() {
    let (_rt, pane) = editor();
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    click(&mut router, &panes, 60, 70);
    type_text(&mut router, &panes, "one");
    router.route(&key(KeyCode::Enter, KeyModifiers::NONE), &panes);
    type_text(&mut router, &panes, "two");
    assert_eq!(pane.component().get_body(), "one\ntwo");
    let (c, r) = cell(60, 70);
    assert_eq!(router.route(&mouse(MouseEventKind::ScrollDown, c, r), &panes), Routed::Consumed);
}

#[test]
fn combo_box_popup_selects_an_item() {
    let (_rt, pane) = editor();
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    assert_eq!(pane.component().get_choice(), "one");
    // Open the popup, then pick an item below the box. The popup lives inside the pane.
    click(&mut router, &panes, 60, 155);
    let picked = (175..=260).step_by(20).any(|y| {
        click(&mut router, &panes, 60, y);
        if pane.component().get_choice() != "one" {
            return true;
        }
        click(&mut router, &panes, 60, 155);
        false
    });
    assert!(picked, "no ComboBox item could be picked; choice = {}", pane.component().get_choice());

    // Keyboard works too once the box has focus.
    let before = pane.component().get_choice();
    router.route(&key(KeyCode::Down, KeyModifiers::NONE), &panes);
    router.route(&key(KeyCode::Up, KeyModifiers::NONE), &panes);
    router.route(&key(KeyCode::Up, KeyModifiers::NONE), &panes);
    assert_ne!(pane.component().get_choice(), before);
}

#[test]
fn drag_leaving_the_pane_keeps_capture() {
    let (_rt, pane) = editor();
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    let (c, r) = cell(300, 50);
    assert_eq!(router.route(&mouse(MouseEventKind::Down(MouseButton::Left), c, r), &panes), Routed::Consumed);
    // Drag to column 0, left of the pane: still delivered, at a negative position.
    assert_eq!(router.route(&mouse(MouseEventKind::Drag(MouseButton::Left), 0, r), &panes), Routed::Consumed);
    assert!(pane.component().get_drag_x() < 0.0, "drag-x = {}", pane.component().get_drag_x());
    assert_eq!(router.route(&mouse(MouseEventKind::Up(MouseButton::Left), 0, r), &panes), Routed::Consumed);
    assert_eq!(pane.component().get_ups(), 1);
    // Capture ended: the next click outside is ratatui's.
    assert_eq!(router.route(&mouse(MouseEventKind::Down(MouseButton::Left), 0, r), &panes), Routed::Unhandled);
}

#[test]
fn popup_closes_on_click_outside_the_pane() {
    let (_rt, pane) = editor();
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    click(&mut router, &panes, 60, 25); // focus the pane
    pane.component().invoke_open_popup();
    click(&mut router, &panes, 300, 190);
    assert_eq!(pane.component().get_popup_hits(), 1, "click inside the open popup");

    pane.component().invoke_open_popup();
    // Click outside the pane: ratatui gets it, and the popup closes.
    assert_eq!(
        router.route(&mouse(MouseEventKind::Down(MouseButton::Left), 0, 0), &panes),
        Routed::Unhandled
    );
    router.route(&mouse(MouseEventKind::Up(MouseButton::Left), 0, 0), &panes);
    assert_eq!(router.focused(), None);
    click(&mut router, &panes, 300, 190);
    assert_eq!(pane.component().get_popup_hits(), 1, "popup should be closed");
    assert_eq!(pane.component().get_under_hits(), 1, "click should reach the item under the popup");
}

#[test]
fn rejected_keys_fall_back_to_ratatui() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Gauge::new).unwrap();
    pane.set_area(Rect::new(0, 0, 20, 5));
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];

    // Nothing focused: everything is ratatui's.
    assert_eq!(router.route(&key(KeyCode::Char('+'), KeyModifiers::NONE), &panes), Routed::Unhandled);
    router.focus(pane.id(), &panes);
    assert_eq!(router.route(&key(KeyCode::Char('+'), KeyModifiers::NONE), &panes), Routed::Consumed);
    assert!((pane.component().get_value() - 0.3).abs() < 1e-5);
    assert_eq!(router.route(&key(KeyCode::Char('x'), KeyModifiers::NONE), &panes), Routed::Unhandled);
    assert_eq!(router.route(&key(KeyCode::F(5), KeyModifiers::NONE), &panes), Routed::Unhandled);

    // Slint keeps Tab for its own focus chain even when the FocusScope rejects it.
    assert_eq!(router.route(&key(KeyCode::Tab, KeyModifiers::NONE), &panes), Routed::Consumed);

    // The reserved exit key leaves the pane before Slint sees it.
    router.focus(pane.id(), &panes);
    assert_eq!(
        router.route(&key(KeyCode::Char('g'), KeyModifiers::CONTROL), &panes),
        Routed::FocusReleased(pane.id())
    );
    assert_eq!(router.focused(), None);
    assert_eq!(router.route(&key(KeyCode::Char('+'), KeyModifiers::NONE), &panes), Routed::Unhandled);
}

#[test]
fn key_kinds_follow_the_keyboard_protocol() {
    let rt = Runtime::new().unwrap();
    let mut pane = rt.create_pane(&halfblocks(), Gauge::new).unwrap();
    pane.set_area(Rect::new(0, 0, 20, 5));
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 1] = [&pane];
    router.focus(pane.id(), &panes);
    let g = pane.component();

    // Legacy terminals report presses only: each press gets a synthetic release.
    router.route(&key(KeyCode::Char('x'), KeyModifiers::NONE), &panes);
    assert_eq!((g.get_presses(), g.get_releases()), (1, 1));

    // With the kitty keyboard protocol, releases and repeats come from the terminal.
    router.set_keyboard_enhanced(true);
    router.route(&key_kind(KeyCode::Char('x'), KeyEventKind::Press), &panes);
    assert_eq!((g.get_presses(), g.get_releases()), (2, 1));
    router.route(&key_kind(KeyCode::Char('x'), KeyEventKind::Repeat), &panes);
    assert_eq!(g.get_presses(), 3, "a repeat is a key-pressed for Slint");
    router.route(&key_kind(KeyCode::Char('x'), KeyEventKind::Release), &panes);
    assert_eq!((g.get_presses(), g.get_releases()), (3, 2));
}

#[test]
fn hover_and_pointer_outside_panes() {
    let rt = Runtime::new().unwrap();
    let mut a = rt.create_pane(&halfblocks(), Gauge::new).unwrap();
    let mut b = rt.create_pane(&halfblocks(), Gauge::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 5));
    b.set_area(Rect::new(10, 0, 10, 5));
    let mut router = InputRouter::new();
    let panes: [&dyn PaneInput; 2] = [&a, &b];
    assert_eq!(router.route(&mouse(MouseEventKind::Moved, 2, 2), &panes), Routed::Consumed);
    assert_eq!(router.route(&mouse(MouseEventKind::Moved, 12, 2), &panes), Routed::Consumed);
    assert_eq!(router.route(&mouse(MouseEventKind::Moved, 30, 2), &panes), Routed::Unhandled);

    // Clicking moves focus between panes.
    router.route(&mouse(MouseEventKind::Down(MouseButton::Left), 2, 2), &panes);
    router.route(&mouse(MouseEventKind::Up(MouseButton::Left), 2, 2), &panes);
    assert_eq!(router.focused(), Some(a.id()));
    assert_eq!(a.component().get_clicks(), 1);
    router.route(&mouse(MouseEventKind::Down(MouseButton::Left), 12, 2), &panes);
    router.route(&mouse(MouseEventKind::Up(MouseButton::Left), 12, 2), &panes);
    assert_eq!(router.focused(), Some(b.id()));
    assert_eq!(b.component().get_clicks(), 1);

    // A dropped pane is forgotten.
    drop(b);
    let panes: [&dyn PaneInput; 1] = [&a];
    assert_eq!(router.route(&key(KeyCode::Char('+'), KeyModifiers::NONE), &panes), Routed::Unhandled);
    assert_eq!(router.focused(), None);
}

slint::slint! {
    export component KeysWithTouchArea inherits Window {
        in-out property <int> hits;
        FocusScope {
            key-pressed(e) => { if (e.text == "+") { root.hits += 1; return accept; } reject }
            TouchArea {}
        }
    }
    export component KeysOnly inherits Window {
        in-out property <int> hits;
        FocusScope {
            key-pressed(e) => { if (e.text == "+") { root.hits += 1; return accept; } reject }
        }
    }
}

/// Slint behaviour the examples rely on: a click focuses a FocusScope only when no TouchArea
/// inside it takes the click. Focusing the pane does not focus anything inside it.
#[test]
fn click_focuses_a_focus_scope_only_without_inner_touch_area() {
    let rt = Runtime::new().unwrap();
    let send = |pane: &dyn PaneInput| {
        let mut router = InputRouter::new();
        let panes: [&dyn PaneInput; 1] = [pane];
        router.route(&mouse(MouseEventKind::Down(MouseButton::Left), 3, 2), &panes);
        router.route(&mouse(MouseEventKind::Up(MouseButton::Left), 3, 2), &panes);
        router.route(&key(KeyCode::Char('+'), KeyModifiers::NONE), &panes)
    };
    let mut with = rt.create_pane(&halfblocks(), KeysWithTouchArea::new).unwrap();
    with.set_area(Rect::new(0, 0, 20, 5));
    assert_eq!(send(&with), Routed::Unhandled);
    assert_eq!(with.component().get_hits(), 0);

    let mut only = rt.create_pane(&halfblocks(), KeysOnly::new).unwrap();
    only.set_area(Rect::new(0, 0, 20, 5));
    assert_eq!(send(&only), Routed::Consumed);
    assert_eq!(only.component().get_hits(), 1);
}
