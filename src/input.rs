//! Routes crossterm input to the pane under the pointer or in focus.
//!
//! - Pointer presses capture the pointer: drags and the release go to the pressed pane even when
//!   the pointer leaves its area.
//! - Keys go to the focused pane. Keys Slint rejects come back as [`Routed::Unhandled`] for
//!   ratatui. Slint itself keeps Tab, Backtab and Escape, so a host-reserved key (default
//!   Ctrl+G) leaves the pane before Slint sees it.
//! - Modifiers held on a key event are sent to Slint as modifier key presses around it. Without
//!   the kitty keyboard protocol every press gets a synthetic release.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;
use slint::platform::{Key, PointerEventButton, WindowEvent, WindowEventDispatchResult};
use slint::{LogicalPosition, SharedString};

use crate::pane::{PaneId, PaneInput};

/// What happened to an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Routed {
    /// A pane used it. ratatui should ignore it.
    Consumed,
    /// No pane wanted it. ratatui should handle it.
    Unhandled,
    /// The pane-exit key was pressed; focus left the pane.
    FocusReleased(PaneId),
}

/// A key combination, compared against press events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    /// The key that must be pressed.
    pub code: KeyCode,
    /// The modifiers that must be held, exactly.
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    /// A binding for `code` with exactly `modifiers` held.
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    fn matches(&self, k: &KeyEvent) -> bool {
        let code = match k.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        code == self.code && k.modifiers == self.modifiers
    }
}

/// Default key that leaves a focused pane: Ctrl+G. Tab and Escape cannot do this because Slint
/// consumes them for its own focus chain and popups.
pub const DEFAULT_EXIT_KEY: KeyBinding = KeyBinding::new(KeyCode::Char('g'), KeyModifiers::CONTROL);

const MODIFIERS: [(KeyModifiers, Key); 4] = [
    (KeyModifiers::SHIFT, Key::Shift),
    (KeyModifiers::CONTROL, Key::Control),
    (KeyModifiers::ALT, Key::Alt),
    (KeyModifiers::SUPER, Key::Meta),
];

/// Maps a crossterm key code to the text Slint expects in a key event.
pub fn key_text(code: KeyCode) -> Option<SharedString> {
    use KeyCode as K;
    Some(match code {
        K::Char(c) => c.to_string().into(),
        K::Enter => Key::Return.into(),
        K::Tab => Key::Tab.into(),
        K::BackTab => Key::Backtab.into(),
        K::Backspace => Key::Backspace.into(),
        K::Delete => Key::Delete.into(),
        K::Insert => Key::Insert.into(),
        K::Esc => Key::Escape.into(),
        K::Left => Key::LeftArrow.into(),
        K::Right => Key::RightArrow.into(),
        K::Up => Key::UpArrow.into(),
        K::Down => Key::DownArrow.into(),
        K::Home => Key::Home.into(),
        K::End => Key::End.into(),
        K::PageUp => Key::PageUp.into(),
        K::PageDown => Key::PageDown.into(),
        K::F(n) => match n {
            1 => Key::F1,
            2 => Key::F2,
            3 => Key::F3,
            4 => Key::F4,
            5 => Key::F5,
            6 => Key::F6,
            7 => Key::F7,
            8 => Key::F8,
            9 => Key::F9,
            10 => Key::F10,
            11 => Key::F11,
            12 => Key::F12,
            _ => return None,
        }
        .into(),
        K::Modifier(m) => match m {
            ModifierKeyCode::LeftShift => Key::Shift,
            ModifierKeyCode::RightShift => Key::ShiftR,
            ModifierKeyCode::LeftControl => Key::Control,
            ModifierKeyCode::RightControl => Key::ControlR,
            ModifierKeyCode::LeftAlt => Key::Alt,
            ModifierKeyCode::RightAlt => Key::AltGr,
            ModifierKeyCode::LeftSuper | ModifierKeyCode::LeftMeta => Key::Meta,
            ModifierKeyCode::RightSuper | ModifierKeyCode::RightMeta => Key::MetaR,
            _ => return None,
        }
        .into(),
        _ => return None,
    })
}

/// Converts a cell to a logical position at the cell centre, relative to `area`. Works for cells
/// outside the area (negative offsets), which captured drags need.
pub fn cell_to_logical(
    area: Rect,
    font: ratatui_image::FontSize,
    scale_factor: f32,
    col: u16,
    row: u16,
) -> LogicalPosition {
    let x = (f32::from(col) - f32::from(area.x) + 0.5) * f32::from(font.width);
    let y = (f32::from(row) - f32::from(area.y) + 0.5) * f32::from(font.height);
    LogicalPosition::new(x / scale_factor, y / scale_factor)
}

fn contains(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.right() && row >= area.y && row < area.bottom()
}

fn button(b: MouseButton) -> PointerEventButton {
    match b {
        MouseButton::Left => PointerEventButton::Left,
        MouseButton::Right => PointerEventButton::Right,
        MouseButton::Middle => PointerEventButton::Middle,
    }
}

/// Routes terminal input to panes. One router serves all panes of an application.
pub struct InputRouter {
    focused: Option<PaneId>,
    captured: Option<(PaneId, MouseButton)>,
    hovered: Option<PaneId>,
    /// Modifiers Slint currently believes are held, per the events we sent it.
    held: KeyModifiers,
    exit_key: Option<KeyBinding>,
    keyboard_enhanced: bool,
    scroll_lines: f32,
}

impl Default for InputRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl InputRouter {
    /// A router with no focused pane, [`DEFAULT_EXIT_KEY`] as the exit key and 3 lines per wheel
    /// step.
    pub fn new() -> Self {
        Self {
            focused: None,
            captured: None,
            hovered: None,
            held: KeyModifiers::empty(),
            exit_key: Some(DEFAULT_EXIT_KEY),
            keyboard_enhanced: false,
            scroll_lines: 3.0,
        }
    }

    /// Changes the key that leaves a focused pane. `None` disables it.
    pub fn set_exit_key(&mut self, key: Option<KeyBinding>) {
        self.exit_key = key;
    }

    /// Tells the router the terminal reports key releases (kitty keyboard protocol with
    /// `REPORT_EVENT_TYPES`). Without it, every press is followed by a synthetic release.
    pub fn set_keyboard_enhanced(&mut self, enhanced: bool) {
        self.keyboard_enhanced = enhanced;
    }

    /// Lines scrolled per wheel step.
    pub fn set_scroll_lines(&mut self, lines: f32) {
        self.scroll_lines = lines;
    }

    /// The pane with keyboard focus, if any.
    pub fn focused(&self) -> Option<PaneId> {
        self.focused
    }

    /// Gives a pane keyboard focus, deactivating the previous one.
    pub fn focus(&mut self, id: PaneId, panes: &[&dyn PaneInput]) {
        if self.focused == Some(id) {
            return;
        }
        self.blur(panes);
        if let Some(p) = find(panes, id) {
            p.slint_window()
                .dispatch_event(WindowEvent::WindowActiveChanged(true));
            self.focused = Some(id);
        }
    }

    /// Removes keyboard focus from all panes.
    pub fn blur(&mut self, panes: &[&dyn PaneInput]) {
        if let Some(old) = self.focused.take()
            && let Some(p) = find(panes, old)
        {
            self.release_modifiers(p);
            p.slint_window()
                .dispatch_event(WindowEvent::WindowActiveChanged(false));
        }
    }

    /// Routes one event. Pass every pane that is currently on screen.
    pub fn route(&mut self, event: &Event, panes: &[&dyn PaneInput]) -> Routed {
        // Forget panes that are gone.
        for slot in [&mut self.focused, &mut self.hovered] {
            if slot.is_some_and(|id| find(panes, id).is_none()) {
                *slot = None;
            }
        }
        if self
            .captured
            .is_some_and(|(id, _)| find(panes, id).is_none())
        {
            self.captured = None;
        }
        match event {
            Event::Key(k) => self.route_key(k, panes),
            Event::Mouse(m) => self.route_mouse(m, panes),
            Event::Paste(text) => self.route_paste(text, panes),
            Event::FocusLost => {
                if let Some(p) = self.focused.and_then(|id| find(panes, id)) {
                    self.release_modifiers(p);
                    p.slint_window()
                        .dispatch_event(WindowEvent::WindowActiveChanged(false));
                }
                Routed::Unhandled
            }
            Event::FocusGained => {
                if let Some(p) = self.focused.and_then(|id| find(panes, id)) {
                    p.slint_window()
                        .dispatch_event(WindowEvent::WindowActiveChanged(true));
                }
                Routed::Unhandled
            }
            Event::Resize(..) => Routed::Unhandled,
        }
    }

    fn route_key(&mut self, k: &KeyEvent, panes: &[&dyn PaneInput]) -> Routed {
        let Some(id) = self.focused else {
            return Routed::Unhandled;
        };
        let Some(pane) = find(panes, id) else {
            return Routed::Unhandled;
        };
        if k.kind == KeyEventKind::Press && self.exit_key.is_some_and(|b| b.matches(k)) {
            self.blur(panes);
            return Routed::FocusReleased(id);
        }
        let Some(text) = key_text(k.code) else {
            return Routed::Unhandled;
        };
        let window = pane.slint_window();

        if let KeyCode::Modifier(_) = k.code {
            let event = match k.kind {
                KeyEventKind::Release => WindowEvent::KeyReleased { text },
                _ => WindowEvent::KeyPressed { text },
            };
            window.dispatch_event(event);
            self.held = k.modifiers;
            return Routed::Consumed;
        }

        let result = match k.kind {
            KeyEventKind::Press => {
                self.sync_modifiers(pane, k.modifiers);
                let r = window
                    .dispatch_event_with_result(WindowEvent::KeyPressed { text: text.clone() });
                if !self.keyboard_enhanced {
                    window.dispatch_event(WindowEvent::KeyReleased { text });
                    self.release_modifiers(pane);
                }
                r
            }
            KeyEventKind::Repeat => {
                self.sync_modifiers(pane, k.modifiers);
                window.dispatch_event_with_result(WindowEvent::KeyPressRepeated { text })
            }
            KeyEventKind::Release => {
                let r = window.dispatch_event_with_result(WindowEvent::KeyReleased { text });
                self.sync_modifiers(pane, k.modifiers);
                r
            }
        };
        match result {
            Ok(WindowEventDispatchResult::Accepted) => Routed::Consumed,
            _ => Routed::Unhandled,
        }
    }

    /// Presses and releases Slint modifiers so they match `wanted`.
    fn sync_modifiers(&mut self, pane: &dyn PaneInput, wanted: KeyModifiers) {
        let window = pane.slint_window();
        for (flag, key) in MODIFIERS {
            let want = wanted.contains(flag);
            let have = self.held.contains(flag);
            if want && !have {
                window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
            } else if !want && have {
                window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
            }
        }
        self.held = wanted
            & (KeyModifiers::SHIFT
                | KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER);
    }

    fn release_modifiers(&mut self, pane: &dyn PaneInput) {
        self.sync_modifiers(pane, KeyModifiers::empty());
    }

    fn route_paste(&mut self, text: &str, panes: &[&dyn PaneInput]) -> Routed {
        let Some(pane) = self.focused.and_then(|id| find(panes, id)) else {
            return Routed::Unhandled;
        };
        if !crate::platform::set_clipboard_cache(text.to_owned()) {
            return Routed::Unhandled;
        }
        // Slint pastes from Platform::clipboard_text on its standard paste shortcut.
        self.sync_modifiers(pane, KeyModifiers::CONTROL);
        let window = pane.slint_window();
        let r = window.dispatch_event_with_result(WindowEvent::KeyPressed { text: "v".into() });
        window.dispatch_event(WindowEvent::KeyReleased { text: "v".into() });
        self.release_modifiers(pane);
        match r {
            Ok(WindowEventDispatchResult::Accepted) => Routed::Consumed,
            _ => Routed::Unhandled,
        }
    }

    fn route_mouse(&mut self, m: &MouseEvent, panes: &[&dyn PaneInput]) -> Routed {
        let (col, row) = (m.column, m.row);
        let hit = panes.iter().find(|p| contains(p.area(), col, row)).copied();
        let pos = |p: &dyn PaneInput| {
            cell_to_logical(
                p.area(),
                p.font_size(),
                p.slint_window().scale_factor(),
                col,
                row,
            )
        };

        match m.kind {
            MouseEventKind::Down(b) => {
                if let Some(p) = self.captured.and_then(|(id, _)| find(panes, id)) {
                    p.slint_window()
                        .dispatch_event(WindowEvent::PointerPressed {
                            position: pos(p),
                            button: button(b),
                        });
                    return Routed::Consumed;
                }
                match hit {
                    Some(p) => {
                        self.focus(p.id(), panes);
                        self.set_hover(Some(p), panes);
                        self.captured = Some((p.id(), b));
                        p.slint_window()
                            .dispatch_event(WindowEvent::PointerPressed {
                                position: pos(p),
                                button: button(b),
                            });
                        Routed::Consumed
                    }
                    None => {
                        // A click outside every pane: let the focused pane see it at its
                        // out-of-bounds position so open popups close, then hand focus back.
                        if let Some(p) = self.focused.and_then(|id| find(panes, id)) {
                            let position = pos(p);
                            let w = p.slint_window();
                            w.dispatch_event(WindowEvent::PointerPressed {
                                position,
                                button: button(b),
                            });
                            w.dispatch_event(WindowEvent::PointerReleased {
                                position,
                                button: button(b),
                            });
                            w.dispatch_event(WindowEvent::PointerExited);
                        }
                        self.blur(panes);
                        self.set_hover(None, panes);
                        Routed::Unhandled
                    }
                }
            }
            MouseEventKind::Up(b) => {
                if let Some((id, held)) = self.captured
                    && let Some(p) = find(panes, id)
                {
                    let w = p.slint_window();
                    w.dispatch_event(WindowEvent::PointerReleased {
                        position: pos(p),
                        button: button(b),
                    });
                    if held == b {
                        self.captured = None;
                        if !contains(p.area(), col, row) {
                            // The interaction ended outside: now the pointer has really left.
                            w.dispatch_event(WindowEvent::PointerExited);
                            if self.hovered == Some(id) {
                                self.hovered = None;
                            }
                        }
                    }
                    return Routed::Consumed;
                }
                match hit {
                    Some(p) => {
                        p.slint_window()
                            .dispatch_event(WindowEvent::PointerReleased {
                                position: pos(p),
                                button: button(b),
                            });
                        Routed::Consumed
                    }
                    None => Routed::Unhandled,
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(p) = self.captured.and_then(|(id, _)| find(panes, id)) {
                    // Leaving during a drag is not an exit: Slint keeps its grab.
                    p.slint_window()
                        .dispatch_event(WindowEvent::PointerMoved { position: pos(p) });
                    return Routed::Consumed;
                }
                self.route_move(hit, panes, pos)
            }
            MouseEventKind::Moved => self.route_move(hit, panes, pos),
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                let Some(p) = hit else {
                    return Routed::Unhandled;
                };
                let font = p.font_size();
                let sf = p.slint_window().scale_factor();
                let dy = self.scroll_lines * f32::from(font.height) / sf;
                let dx = self.scroll_lines * f32::from(font.width) / sf;
                let (delta_x, delta_y) = match m.kind {
                    MouseEventKind::ScrollUp => (0.0, dy),
                    MouseEventKind::ScrollDown => (0.0, -dy),
                    MouseEventKind::ScrollLeft => (dx, 0.0),
                    _ => (-dx, 0.0),
                };
                p.slint_window()
                    .dispatch_event(WindowEvent::PointerScrolled {
                        position: pos(p),
                        delta_x,
                        delta_y,
                    });
                Routed::Consumed
            }
        }
    }

    fn route_move(
        &mut self,
        hit: Option<&dyn PaneInput>,
        panes: &[&dyn PaneInput],
        pos: impl Fn(&dyn PaneInput) -> LogicalPosition,
    ) -> Routed {
        self.set_hover(hit, panes);
        match hit {
            Some(p) => {
                p.slint_window()
                    .dispatch_event(WindowEvent::PointerMoved { position: pos(p) });
                Routed::Consumed
            }
            None => Routed::Unhandled,
        }
    }

    fn set_hover(&mut self, hit: Option<&dyn PaneInput>, panes: &[&dyn PaneInput]) {
        let new = hit.map(|p| p.id());
        if self.hovered != new {
            if let Some(old) = self.hovered.and_then(|id| find(panes, id))
                && self.captured.is_none_or(|(id, _)| id != old.id())
            {
                old.slint_window()
                    .dispatch_event(WindowEvent::PointerExited);
            }
            self.hovered = new;
        }
    }
}

fn find<'a>(panes: &[&'a dyn PaneInput], id: PaneId) -> Option<&'a dyn PaneInput> {
    panes.iter().find(|p| p.id() == id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::FontSize;

    #[test]
    fn logical_position_handles_negative_offsets() {
        let area = Rect::new(10, 5, 20, 10);
        let p = cell_to_logical(area, FontSize::new(10, 20), 1.0, 8, 4);
        assert_eq!((p.x, p.y), (-15.0, -10.0));
        let p = cell_to_logical(area, FontSize::new(10, 20), 2.0, 10, 5);
        assert_eq!((p.x, p.y), (2.5, 5.0));
    }

    #[test]
    fn exit_key_ignores_case() {
        let k = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::CONTROL);
        assert!(DEFAULT_EXIT_KEY.matches(&k));
    }
}
