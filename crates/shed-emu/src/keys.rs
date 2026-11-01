//! Turning key presses into the bytes a terminal app expects: xterm's
//! encoding, or the kitty keyboard protocol's once an app turns it on.
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

/// What happened to a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyEvent {
    #[default]
    Press,
    /// Held down: the keyboard repeats it.
    Repeat,
    Release,
}

/// The bytes to send for a key press, or `None` if the terminal ignores it.
pub fn encode(input: KeyInput<'_>, modes: Modes) -> Option<Vec<u8>> {
    encode_event(input, KeyEvent::Press, modes)
}

/// [`encode`] for any key event. Only apps that ask for it through the kitty
/// protocol hear about releases; a repeat is otherwise another press.
pub fn encode_event(input: KeyInput<'_>, event: KeyEvent, modes: Modes) -> Option<Vec<u8>> {
    if modes.kitty != 0
        && let Some(bytes) = kitty(input, event, modes.kitty)
    {
        return (!bytes.is_empty()).then_some(bytes);
    }
    if event == KeyEvent::Release {
        return None;
    }
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

/// The kitty keyboard protocol (<https://sw.kovidgoyal.net/kitty/keyboard-protocol/>):
/// `CSI code ; mods u` for what legacy encoding makes ambiguous, or for
/// every key with flag 8; repeats and releases with flag 2, the shifted
/// key with flag 4. `None` leaves the key to the legacy encoding, which the
/// protocol keeps for the rest; an empty result sends nothing.
fn kitty(input: KeyInput<'_>, event: KeyEvent, flags: u8) -> Option<Vec<u8>> {
    let KeyInput { key, text, mods } = input;
    // Without flag 2 there are only presses.
    let event = match event {
        KeyEvent::Release if flags & 2 == 0 => return Some(Vec::new()),
        KeyEvent::Repeat if flags & 2 == 0 => KeyEvent::Press,
        e => e,
    };
    let all_keys = flags & 8 != 0;
    let with_text = flags & 16 != 0 && event != KeyEvent::Release;
    let alternates = flags & 4 != 0;
    let chorded = mods.ctrl || mods.alt || mods.super_;
    // `;mods` or `;mods:event`, or nothing at all for a plain press.
    let params = |trailing: bool| {
        let e = match event {
            KeyEvent::Press => None,
            KeyEvent::Repeat => Some(2),
            KeyEvent::Release => Some(3),
        };
        match (mods.param(), e) {
            (m, Some(e)) => format!(";{m}:{e}"),
            (1, None) if !trailing => String::new(),
            (m, None) => format!(";{m}"),
        }
    };
    let csi_u = |code: u32, shifted: Option<u32>| {
        let text: Option<String> = text
            .filter(|t| with_text && !t.chars().any(char::is_control))
            .map(|t| t.chars().map(|c| (c as u32).to_string()).collect::<Vec<_>>().join(":"));
        let mut out = format!("\x1b[{code}");
        if let Some(s) = shifted.filter(|s| alternates && mods.shift && *s != code) {
            out.push_str(&format!(":{s}"));
        }
        out.push_str(&params(text.is_some()));
        if let Some(t) = text {
            out.push_str(&format!(";{t}"));
        }
        out.push('u');
        out.into_bytes()
    };
    let functional = |n: u8, final_byte: char| {
        let n = if n == 1 && params(false).is_empty() { String::new() } else { n.to_string() };
        format!("\x1b[{n}{}{final_byte}", params(false)).into_bytes()
    };

    match key {
        "escape" => Some(csi_u(27, None)),
        "enter" | "tab" | "backspace" => {
            // These stay legacy unless every key is an escape, so a shell
            // left in this mode by a crashed program can still run `reset`.
            if !all_keys && event == KeyEvent::Release {
                return Some(Vec::new());
            }
            if !all_keys && !mods.any() {
                return None;
            }
            let code = match key {
                "enter" => 13,
                "tab" => 9,
                _ => 127,
            };
            Some(csi_u(code, None))
        }
        "space" if all_keys || chorded || event == KeyEvent::Release => Some(csi_u(32, None)),
        _ => {
            if let Some((n, final_byte)) = functional_key(key) {
                // The legacy forms already work for a press; repeats, releases
                // and every-key mode spell everything out.
                return match (event, all_keys || mods.any()) {
                    (KeyEvent::Press, false) => None,
                    _ => Some(functional(n, final_byte)),
                };
            }
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                // A named key the protocol has no code for here.
                return None;
            }
            // Shift alone just types: `A` is text, not a chord.
            if !all_keys && !chorded && event != KeyEvent::Release {
                return None;
            }
            let base = c.to_lowercase().next().unwrap_or(c);
            let shifted = text.and_then(single_char).filter(|c| !c.is_control()).unwrap_or(base.to_ascii_uppercase());
            Some(csi_u(base as u32, Some(shifted as u32)))
        }
    }
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Keys the protocol writes as `CSI number ; mods final`: (number, final).
fn functional_key(key: &str) -> Option<(u8, char)> {
    Some(match key {
        "up" => (1, 'A'),
        "down" => (1, 'B'),
        "right" => (1, 'C'),
        "left" => (1, 'D'),
        "home" => (1, 'H'),
        "end" => (1, 'F'),
        "f1" => (1, 'P'),
        "f2" => (1, 'Q'),
        // Not `CSI R`: that reads as a cursor position report.
        "f3" => (13, '~'),
        "f4" => (1, 'S'),
        "insert" => (2, '~'),
        "delete" => (3, '~'),
        "pageup" => (5, '~'),
        "pagedown" => (6, '~'),
        "f5" => (15, '~'),
        "f6" => (17, '~'),
        "f7" => (18, '~'),
        "f8" => (19, '~'),
        "f9" => (20, '~'),
        "f10" => (21, '~'),
        "f11" => (23, '~'),
        "f12" => (24, '~'),
        _ => return None,
    })
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

    fn kitty(flags: u8, key: &str, text: Option<&str>, mods: Mods) -> String {
        let modes = Modes { kitty: flags, ..Default::default() };
        String::from_utf8(encode(KeyInput { key, text, mods }, modes).unwrap()).unwrap()
    }

    #[test]
    fn kitty_disambiguates_what_legacy_cannot() {
        // Plain typing and Shift stay text; Esc and chords become CSI u.
        assert_eq!(kitty(1, "a", Some("a"), Mods::NONE), "a");
        assert_eq!(kitty(1, "a", Some("A"), SHIFT), "A");
        assert_eq!(kitty(1, "escape", None, Mods::NONE), "\x1b[27u");
        assert_eq!(kitty(1, "c", None, CTRL), "\x1b[99;5u");
        assert_eq!(kitty(1, "i", None, CTRL), "\x1b[105;5u", "Ctrl+I is not Tab");
        assert_eq!(kitty(1, "a", Some("A"), Mods { shift: true, ..CTRL }), "\x1b[97;6u");
        assert_eq!(kitty(1, "x", Some("x"), ALT), "\x1b[120;3u");
        assert_eq!(kitty(1, "enter", None, Mods::NONE), "\r");
        assert_eq!(kitty(1, "enter", None, SHIFT), "\x1b[13;2u");
        assert_eq!(kitty(1, "tab", None, SHIFT), "\x1b[9;2u");
        assert_eq!(kitty(1, "backspace", None, CTRL), "\x1b[127;5u");
        assert_eq!(kitty(1, "space", Some(" "), CTRL), "\x1b[32;5u");
        // Named keys keep their legacy forms.
        assert_eq!(kitty(1, "up", None, Mods::NONE), "\x1b[A");
        assert_eq!(kitty(1, "f5", None, CTRL), "\x1b[15;5~");
    }

    #[test]
    fn kitty_can_report_every_key_with_its_text() {
        assert_eq!(kitty(8, "a", Some("a"), Mods::NONE), "\x1b[97u");
        assert_eq!(kitty(8, "enter", None, Mods::NONE), "\x1b[13u");
        assert_eq!(kitty(8 | 16, "a", Some("A"), SHIFT), "\x1b[97;2;65u");
        assert_eq!(kitty(8 | 16, "e", Some("é"), Mods::NONE), "\x1b[101;1;233u");
        let app_cursor = Modes { kitty: 8, app_cursor: true, ..Default::default() };
        assert_eq!(encode(KeyInput { key: "up", text: None, mods: Mods::NONE }, app_cursor).unwrap(), b"\x1b[A");
    }

    fn kitty_event(flags: u8, key: &str, text: Option<&str>, mods: Mods, event: KeyEvent) -> Option<String> {
        let modes = Modes { kitty: flags, ..Default::default() };
        encode_event(KeyInput { key, text, mods }, event, modes).map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn kitty_reports_repeats_and_releases_when_asked() {
        use KeyEvent::{Release, Repeat};
        let s = |t: &str| Some(t.to_string());
        // Only with flag 2; otherwise a repeat is a press and a release is nothing.
        assert_eq!(kitty_event(1, "escape", None, Mods::NONE, Release), None);
        assert_eq!(kitty_event(1, "c", None, CTRL, Repeat), s("\x1b[99;5u"));
        assert_eq!(kitty_event(0, "a", Some("a"), Mods::NONE, Release), None);
        assert_eq!(kitty_event(0, "a", Some("a"), Mods::NONE, Repeat), s("a"));

        assert_eq!(kitty_event(3, "escape", None, Mods::NONE, Release), s("\x1b[27;1:3u"));
        assert_eq!(kitty_event(3, "c", None, CTRL, Repeat), s("\x1b[99;5:2u"));
        // Typing stays text; its release is spelled out.
        assert_eq!(kitty_event(3, "a", Some("a"), Mods::NONE, Repeat), s("a"));
        assert_eq!(kitty_event(3, "a", None, Mods::NONE, Release), s("\x1b[97;1:3u"));
        assert_eq!(kitty_event(3, "space", None, Mods::NONE, Release), s("\x1b[32;1:3u"));
        // Enter, Tab and Backspace keep quiet on release unless every key is reported.
        assert_eq!(kitty_event(3, "enter", None, Mods::NONE, Release), None);
        assert_eq!(kitty_event(3, "enter", None, Mods::NONE, Repeat), s("\r"));
        assert_eq!(kitty_event(11, "enter", None, Mods::NONE, Release), s("\x1b[13;1:3u"));
        // Named keys: legacy for a plain press, spelled out otherwise.
        assert_eq!(kitty_event(3, "up", None, Mods::NONE, KeyEvent::Press), s("\x1b[A"));
        assert_eq!(kitty_event(3, "up", None, Mods::NONE, Release), s("\x1b[1;1:3A"));
        assert_eq!(kitty_event(3, "pageup", None, CTRL, Repeat), s("\x1b[5;5:2~"));
        assert_eq!(kitty_event(3, "f3", None, Mods::NONE, Release), s("\x1b[13;1:3~"));
        assert_eq!(kitty_event(8, "f1", None, Mods::NONE, KeyEvent::Press), s("\x1b[P"));
    }

    #[test]
    fn kitty_alternate_keys_add_the_shifted_key() {
        let shift_ctrl = Mods { shift: true, ..CTRL };
        assert_eq!(kitty(5, "a", Some("A"), shift_ctrl), "\x1b[97:65;6u");
        // No text with Ctrl: an ASCII letter's shifted key is still known.
        assert_eq!(kitty(5, "a", None, shift_ctrl), "\x1b[97:65;6u");
        assert_eq!(kitty(5, "a", Some("\x01"), shift_ctrl), "\x1b[97:65;6u");
        assert_eq!(kitty(13, "1", Some("!"), SHIFT), "\x1b[49:33;2u");
        // Without Shift, or without flag 4, there is none.
        assert_eq!(kitty(5, "a", None, CTRL), "\x1b[97;5u");
        assert_eq!(kitty(1, "a", Some("A"), shift_ctrl), "\x1b[97;6u");
    }

    #[test]
    fn paste() {
        let plain = Modes::default();
        assert_eq!(encode_paste("a\nb", plain), b"a\rb");
        let bracketed = Modes { bracketed_paste: true, ..Default::default() };
        assert_eq!(encode_paste("x\x1b[201~y", bracketed), b"\x1b[200~xy\x1b[201~");
    }
}
