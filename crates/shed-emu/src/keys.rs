//! Turning key presses into the bytes a terminal app expects (xterm encoding).
//!
//! Key names follow GPUI's: `enter`, `pageup`, `f5`, `a`, `/`. The encoder is
//! kept free of GPUI types so it can be tested without a window.

use crate::Modes;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, super_: false };

    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.super_
    }

    /// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl + 8·super.
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8 + 8 * self.super_ as u8
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KeyInput<'a> {
    /// The key's name, as GPUI reports it.
    pub key: &'a str,
    /// The text the key types with the current layout and modifiers, if any.
    pub text: Option<&'a str>,
    pub mods: Mods,
}

/// The bytes to send for a key press, or `None` if the terminal ignores it.
pub fn encode(input: KeyInput<'_>, modes: Modes) -> Option<Vec<u8>> {
    let KeyInput { key, text, mods } = input;

    if let Some(bytes) = special(key, mods, modes) {
        return Some(bytes);
    }

    if mods.ctrl
        && !mods.super_
        && let Some(byte) = ctrl_byte(key)
    {
        let mut out = Vec::with_capacity(2);
        if mods.alt {
            out.push(0x1b);
        }
        out.push(byte);
        return Some(out);
    }

    if mods.super_ {
        return None;
    }

    let text = text.filter(|t| !t.is_empty())?;
    let mut out = Vec::with_capacity(text.len() + 1);
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(text.as_bytes());
    Some(out)
}

/// Keys with no printable text: arrows, function keys, editing keys.
fn special(key: &str, mods: Mods, modes: Modes) -> Option<Vec<u8>> {
    let esc_alt = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if mods.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };

    Some(match key {
        "enter" => esc_alt(b"\r"),
        "tab" if mods.shift => b"\x1b[Z".to_vec(),
        "tab" => esc_alt(b"\t"),
        "escape" => esc_alt(b"\x1b"),
        "backspace" if mods.ctrl => esc_alt(b"\x08"),
        "backspace" => esc_alt(b"\x7f"),
        "space" if mods.ctrl => esc_alt(b"\x00"),
        "space" => esc_alt(b" "),

        "up" => cursor(b'A', mods, modes),
        "down" => cursor(b'B', mods, modes),
        "right" => cursor(b'C', mods, modes),
        "left" => cursor(b'D', mods, modes),
        "home" => cursor(b'H', mods, modes),
        "end" => cursor(b'F', mods, modes),

        "insert" => tilde(2, mods),
        "delete" => tilde(3, mods),
        "pageup" => tilde(5, mods),
        "pagedown" => tilde(6, mods),

        "f1" => ss3_or_csi(b'P', mods),
        "f2" => ss3_or_csi(b'Q', mods),
        "f3" => ss3_or_csi(b'R', mods),
        "f4" => ss3_or_csi(b'S', mods),
        "f5" => tilde(15, mods),
        "f6" => tilde(17, mods),
        "f7" => tilde(18, mods),
        "f8" => tilde(19, mods),
        "f9" => tilde(20, mods),
        "f10" => tilde(21, mods),
        "f11" => tilde(23, mods),
        "f12" => tilde(24, mods),
        _ => return None,
    })
}

/// Arrows, Home and End: `ESC [ X`, `ESC O X` in application cursor mode,
/// `ESC [ 1 ; m X` with modifiers.
fn cursor(final_byte: u8, mods: Mods, modes: Modes) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{}", mods.param(), final_byte as char).into_bytes()
    } else if modes.app_cursor {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn tilde(n: u8, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{n};{}~", mods.param()).into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn ss3_or_csi(final_byte: u8, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{}", mods.param(), final_byte as char).into_bytes()
    } else {
        vec![0x1b, b'O', final_byte]
    }
}

/// The C0 control byte for Ctrl plus `key`, following xterm's legacy table.
fn ctrl_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '`' | '~' => 0x1e,
        '_' | '7' | '/' | '-' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// Wraps pasted text for the PTY: bracketed if the app asked, with newlines as CR.
pub fn encode_paste(text: &str, modes: Modes) -> Vec<u8> {
    // Strip the paste-end marker so a paste cannot break out of the bracket.
    let text = text.replace("\x1b[201~", "");
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str, text: Option<&str>, mods: Mods) -> Option<Vec<u8>> {
        encode(KeyInput { key, text, mods }, Modes::default())
    }

    const CTRL: Mods = Mods { ctrl: true, ..Mods::NONE };
    const ALT: Mods = Mods { alt: true, ..Mods::NONE };
    const SHIFT: Mods = Mods { shift: true, ..Mods::NONE };

    #[test]
    fn printable_text() {
        assert_eq!(key("a", Some("a"), Mods::NONE).unwrap(), b"a");
        assert_eq!(key("a", Some("A"), SHIFT).unwrap(), b"A");
        assert_eq!(key("/", Some("?"), Mods::NONE).unwrap(), b"?");
        assert_eq!(key("e", Some("é"), Mods::NONE).unwrap(), "é".as_bytes());
        assert_eq!(key("x", Some("x"), ALT).unwrap(), b"\x1bx");
    }

    #[test]
    fn control_keys() {
        assert_eq!(key("c", None, CTRL).unwrap(), b"\x03");
        assert_eq!(key("c", None, Mods { shift: true, ..CTRL }).unwrap(), b"\x03");
        assert_eq!(key("[", None, CTRL).unwrap(), b"\x1b");
        assert_eq!(key("space", None, CTRL).unwrap(), b"\x00");
        assert_eq!(key("a", None, Mods { alt: true, ..CTRL }).unwrap(), b"\x1b\x01");
        assert_eq!(key("backspace", None, Mods::NONE).unwrap(), b"\x7f");
        assert_eq!(key("backspace", None, ALT).unwrap(), b"\x1b\x7f");
        assert_eq!(key("enter", None, Mods::NONE).unwrap(), b"\r");
        assert_eq!(key("tab", None, SHIFT).unwrap(), b"\x1b[Z");
    }

    #[test]
    fn cursor_keys_follow_mode_and_modifiers() {
        assert_eq!(key("up", None, Mods::NONE).unwrap(), b"\x1b[A");
        let app = Modes { app_cursor: true, ..Default::default() };
        let up = encode(KeyInput { key: "up", text: None, mods: Mods::NONE }, app).unwrap();
        assert_eq!(up, b"\x1bOA");
        assert_eq!(key("up", None, CTRL).unwrap(), b"\x1b[1;5A");
        assert_eq!(key("up", None, Mods { shift: true, ..CTRL }).unwrap(), b"\x1b[1;6A");
        assert_eq!(key("left", None, ALT).unwrap(), b"\x1b[1;3D");
    }

    #[test]
    fn function_and_editing_keys() {
        assert_eq!(key("f1", None, Mods::NONE).unwrap(), b"\x1bOP");
        assert_eq!(key("f5", None, Mods::NONE).unwrap(), b"\x1b[15~");
        assert_eq!(key("f5", None, SHIFT).unwrap(), b"\x1b[15;2~");
        assert_eq!(key("delete", None, Mods::NONE).unwrap(), b"\x1b[3~");
        assert_eq!(key("pageup", None, CTRL).unwrap(), b"\x1b[5;5~");
    }

    #[test]
    fn super_combinations_are_left_to_shed() {
        assert_eq!(key("c", Some("c"), Mods { super_: true, ..Mods::NONE }), None);
    }

    #[test]
    fn paste() {
        let plain = Modes::default();
        assert_eq!(encode_paste("a\nb", plain), b"a\rb");
        let bracketed = Modes { bracketed_paste: true, ..Default::default() };
        assert_eq!(encode_paste("x\x1b[201~y", bracketed), b"\x1b[200~xy\x1b[201~");
    }
}
