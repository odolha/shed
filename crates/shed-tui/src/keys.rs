//! Keys from the terminal (crossterm's events) in the form `shed_emu::keys`
//! encodes, so a pane gets what the windows would send: xterm's encoding,
//! or the kitty protocol's when its program asked for it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use shed_emu::Modes;
use shed_emu::keys::{self, KeyInput, Mods};

/// A key as the encoder names it (`enter`, `f5`, `a`), with the text it types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub key: String,
    pub text: Option<String>,
    pub mods: Mods,
}

pub fn mods(m: KeyModifiers) -> Mods {
    Mods {
        ctrl: m.contains(KeyModifiers::CONTROL),
        alt: m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::META),
        shift: m.contains(KeyModifiers::SHIFT),
        super_: m.contains(KeyModifiers::SUPER),
    }
}

/// The key's name and text, or `None` for keys a pane has no use for.
pub fn named(ev: &KeyEvent) -> Option<Named> {
    let mut mods = mods(ev.modifiers);
    let plain = |k: &str| Some((k.to_string(), None));
    let (key, text) = match ev.code {
        KeyCode::Char(' ') => ("space".to_string(), (!mods.ctrl).then(|| " ".to_string())),
        KeyCode::Char(c) => {
            // Shift is in the character already ('A'); the name is the key's ('a').
            let key = c.to_lowercase().collect::<String>();
            if c.is_uppercase() {
                mods.shift = true;
            }
            (key, (!mods.ctrl && !mods.super_).then(|| c.to_string()))
        }
        KeyCode::Enter => plain("enter")?,
        KeyCode::Tab => plain("tab")?,
        KeyCode::BackTab => {
            mods.shift = true;
            plain("tab")?
        }
        KeyCode::Backspace => plain("backspace")?,
        KeyCode::Esc => plain("escape")?,
        KeyCode::Up => plain("up")?,
        KeyCode::Down => plain("down")?,
        KeyCode::Left => plain("left")?,
        KeyCode::Right => plain("right")?,
        KeyCode::Home => plain("home")?,
        KeyCode::End => plain("end")?,
        KeyCode::PageUp => plain("pageup")?,
        KeyCode::PageDown => plain("pagedown")?,
        KeyCode::Insert => plain("insert")?,
        KeyCode::Delete => plain("delete")?,
        KeyCode::F(n) if (1..=12).contains(&n) => (format!("f{n}"), None),
        _ => return None,
    };
    Some(Named { key, text, mods })
}

/// The bytes a pane in `modes` gets for this key event, if any.
pub fn encode(ev: &KeyEvent, modes: Modes) -> Option<Vec<u8>> {
    let event = match ev.kind {
        KeyEventKind::Press => keys::KeyEvent::Press,
        KeyEventKind::Repeat => keys::KeyEvent::Repeat,
        KeyEventKind::Release => keys::KeyEvent::Release,
    };
    let n = named(ev)?;
    keys::encode_event(KeyInput { key: &n.key, text: n.text.as_deref(), mods: n.mods }, event, modes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    fn bytes(code: KeyCode, m: KeyModifiers, modes: Modes) -> Vec<u8> {
        encode(&key(code, m), modes).unwrap_or_default()
    }

    #[test]
    fn typing_and_control_keys() {
        let m = Modes::default();
        assert_eq!(bytes(KeyCode::Char('a'), KeyModifiers::NONE, m), b"a");
        assert_eq!(bytes(KeyCode::Char('A'), KeyModifiers::SHIFT, m), b"A");
        assert_eq!(bytes(KeyCode::Char('é'), KeyModifiers::NONE, m), "é".as_bytes());
        assert_eq!(bytes(KeyCode::Char('c'), KeyModifiers::CONTROL, m), [3]);
        assert_eq!(bytes(KeyCode::Char('b'), KeyModifiers::ALT, m), b"\x1bb");
        assert_eq!(bytes(KeyCode::Char(' '), KeyModifiers::NONE, m), b" ");
        assert_eq!(bytes(KeyCode::Char(' '), KeyModifiers::CONTROL, m), [0]);
        assert_eq!(bytes(KeyCode::Enter, KeyModifiers::NONE, m), b"\r");
        assert_eq!(bytes(KeyCode::Backspace, KeyModifiers::NONE, m), [0x7f]);
        assert_eq!(bytes(KeyCode::Esc, KeyModifiers::NONE, m), [0x1b]);
        assert_eq!(bytes(KeyCode::BackTab, KeyModifiers::SHIFT, m), b"\x1b[Z");
        assert_eq!(bytes(KeyCode::F(5), KeyModifiers::NONE, m), b"\x1b[15~");
        assert_eq!(bytes(KeyCode::Delete, KeyModifiers::NONE, m), b"\x1b[3~");
    }

    #[test]
    fn arrows_follow_the_cursor_mode() {
        let m = Modes::default();
        assert_eq!(bytes(KeyCode::Up, KeyModifiers::NONE, m), b"\x1b[A");
        assert_eq!(bytes(KeyCode::Right, KeyModifiers::CONTROL, m), b"\x1b[1;5C");
        let app = Modes { app_cursor: true, ..Modes::default() };
        assert_eq!(bytes(KeyCode::Up, KeyModifiers::NONE, app), b"\x1bOA");
    }

    #[test]
    fn the_kitty_protocol_when_asked() {
        let kitty = Modes { kitty: 1, ..Modes::default() };
        assert_eq!(bytes(KeyCode::Esc, KeyModifiers::NONE, kitty), b"\x1b[27u");
        assert_eq!(bytes(KeyCode::Char('c'), KeyModifiers::CONTROL, kitty), b"\x1b[99;5u");
        // Plain typing stays text.
        assert_eq!(bytes(KeyCode::Char('x'), KeyModifiers::NONE, kitty), b"x");
        assert_eq!(bytes(KeyCode::Enter, KeyModifiers::SHIFT, kitty), b"\x1b[13;2u");
    }

    #[test]
    fn releases_only_reach_programs_that_ask() {
        let mut ev = key(KeyCode::Char('a'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert_eq!(encode(&ev, Modes::default()), None);
        let events = Modes { kitty: 2 | 8, ..Modes::default() };
        assert_eq!(encode(&ev, events).unwrap(), b"\x1b[97;1:3u");
    }
}
