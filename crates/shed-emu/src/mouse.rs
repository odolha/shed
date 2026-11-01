//! Mouse reports for apps that ask for them (vim, htop, lazygit).

use crate::keys::Mods;
use crate::{Modes, MouseMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press(Button),
    Release(Button),
    /// Motion, with the button held if any.
    Move(Option<Button>),
}

/// The report for a mouse action at a 0-based cell, or `None` if the app's
/// mouse mode does not want it.
pub fn encode(action: MouseAction, col: usize, row: usize, mods: Mods, modes: Modes) -> Option<Vec<u8>> {
    let wanted = match (action, modes.mouse) {
        (_, MouseMode::Off) => false,
        (MouseAction::Move(None), mode) => mode == MouseMode::Motion,
        (MouseAction::Move(Some(_)), mode) => mode != MouseMode::Click,
        _ => true,
    };
    if !wanted {
        return None;
    }

    let button_code = |b: Button| match b {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    let mut code: u32 = match action {
        MouseAction::Press(b) => button_code(b),
        MouseAction::Release(b) if modes.sgr_mouse => button_code(b),
        MouseAction::Release(_) => 3,
        MouseAction::Move(Some(b)) => button_code(b) + 32,
        MouseAction::Move(None) => 3 + 32,
    };
    if matches!(action, MouseAction::Release(Button::WheelUp | Button::WheelDown)) {
        return None;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;

    let (x, y) = (col + 1, row + 1);
    if modes.sgr_mouse {
        let end = if matches!(action, MouseAction::Release(_)) { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }

    let mut out = b"\x1b[M".to_vec();
    let mut push = |v: usize| -> Option<()> {
        let v = 32 + v;
        if modes.utf8_mouse {
            let c = char::from_u32(v as u32)?;
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        } else {
            // The legacy encoding cannot express positions past 223.
            out.push(u8::try_from(v).ok()?);
        }
        Some(())
    };
    push(code as usize)?;
    push(x)?;
    push(y)?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(mouse: MouseMode, sgr: bool) -> Modes {
        Modes { mouse, sgr_mouse: sgr, ..Default::default() }
    }

    #[test]
    fn off_sends_nothing() {
        let m = modes(MouseMode::Off, true);
        assert_eq!(encode(MouseAction::Press(Button::Left), 0, 0, Mods::NONE, m), None);
    }

    #[test]
    fn sgr_press_release_and_wheel() {
        let m = modes(MouseMode::Click, true);
        assert_eq!(encode(MouseAction::Press(Button::Left), 4, 2, Mods::NONE, m).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(encode(MouseAction::Release(Button::Left), 4, 2, Mods::NONE, m).unwrap(), b"\x1b[<0;5;3m");
        let ctrl = Mods { ctrl: true, ..Mods::NONE };
        assert_eq!(encode(MouseAction::Press(Button::WheelDown), 0, 0, ctrl, m).unwrap(), b"\x1b[<81;1;1M");
        assert_eq!(encode(MouseAction::Move(Some(Button::Left)), 1, 1, Mods::NONE, m), None);
    }

    #[test]
    fn drag_and_motion_modes() {
        let drag = modes(MouseMode::Drag, true);
        assert_eq!(encode(MouseAction::Move(Some(Button::Left)), 1, 1, Mods::NONE, drag).unwrap(), b"\x1b[<32;2;2M");
        assert_eq!(encode(MouseAction::Move(None), 1, 1, Mods::NONE, drag), None);
        let motion = modes(MouseMode::Motion, true);
        assert_eq!(encode(MouseAction::Move(None), 1, 1, Mods::NONE, motion).unwrap(), b"\x1b[<35;2;2M");
    }

    #[test]
    fn legacy_encoding() {
        let m = modes(MouseMode::Click, false);
        assert_eq!(encode(MouseAction::Press(Button::Left), 0, 0, Mods::NONE, m).unwrap(), b"\x1b[M !!");
        assert_eq!(encode(MouseAction::Release(Button::Left), 0, 0, Mods::NONE, m).unwrap(), b"\x1b[M#!!");
        assert_eq!(encode(MouseAction::Press(Button::Left), 300, 0, Mods::NONE, m), None);
    }
}
