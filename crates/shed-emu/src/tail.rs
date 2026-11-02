//! The unfinished end of an output stream: an escape sequence or UTF-8
//! character the parser has started but not completed.
//!
//! A replay is built from the emulator's state, which a half-read sequence has
//! not changed yet; the parser holds it. A fresh emulator restored from the
//! replay would start the rest of that sequence in its ground state and print
//! it as text. So the replay ends with these bytes, and the rest of the
//! sequence, arriving later, completes it there too.

/// Strings (OSC, DCS…) longer than this are not kept; a replay taken in the
/// middle of one loses it.
const MAX: usize = 1 << 20;

#[derive(Default)]
pub struct Tail {
    state: State,
    pending: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum State {
    #[default]
    Ground,
    /// After ESC, or ESC and intermediates.
    Escape,
    Csi,
    /// OSC, DCS, APC, PM or SOS, until BEL or ST.
    Str,
    /// ESC inside a string: ST if `\` follows.
    StrEscape,
    /// A UTF-8 character with this many bytes still to come.
    Utf8(u8),
    /// A string too long to keep, until it ends.
    Skip,
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

impl Tail {
    /// The bytes of the unfinished sequence, if any.
    pub fn pending(&self) -> &[u8] {
        if matches!(self.state, State::Ground | State::Skip) { &[] } else { &self.pending }
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
        if self.pending.len() > MAX {
            self.pending = Vec::new();
            self.state = State::Skip;
        }
    }

    fn start(&mut self, state: State, b: u8) {
        self.pending.clear();
        self.pending.push(b);
        self.state = state;
    }

    fn end(&mut self) {
        self.pending.clear();
        self.state = State::Ground;
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Ground => match b {
                ESC => self.start(State::Escape, b),
                0xc0..=0xdf => self.start(State::Utf8(1), b),
                0xe0..=0xef => self.start(State::Utf8(2), b),
                0xf0..=0xf7 => self.start(State::Utf8(3), b),
                _ => {}
            },
            State::Utf8(left) => match b {
                0x80..=0xbf if left > 1 => {
                    self.pending.push(b);
                    self.state = State::Utf8(left - 1);
                }
                0x80..=0xbf => self.end(),
                // An invalid sequence ends here; this byte starts afresh.
                _ => {
                    self.end();
                    self.byte(b);
                }
            },
            State::Escape => match b {
                ESC => self.start(State::Escape, b),
                CAN | SUB => self.end(),
                // Other controls run at once and are not part of the sequence.
                0x00..=0x1f => {}
                b'[' => self.push(State::Csi, b),
                b']' | b'P' | b'_' | b'^' | b'X' => self.push(State::Str, b),
                0x20..=0x2f => self.pending.push(b),
                _ => self.end(),
            },
            State::Csi => match b {
                ESC => self.start(State::Escape, b),
                CAN | SUB => self.end(),
                0x00..=0x1f | 0x7f => {}
                0x40..=0x7e => self.end(),
                _ => self.pending.push(b),
            },
            State::Str => match b {
                BEL | CAN | SUB => self.end(),
                ESC => self.push(State::StrEscape, b),
                _ => self.pending.push(b),
            },
            State::StrEscape => match b {
                b'\\' => self.end(),
                // ESC ended the string; it starts a new sequence.
                _ => {
                    self.start(State::Escape, ESC);
                    self.byte(b);
                }
            },
            State::Skip => match b {
                BEL | CAN | SUB => self.end(),
                ESC => self.start(State::StrEscape, b),
                _ => {}
            },
        }
    }

    fn push(&mut self, state: State, b: u8) {
        self.pending.push(b);
        self.state = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(bytes: &[u8]) -> Vec<u8> {
        let mut t = Tail::default();
        t.advance(bytes);
        t.pending().to_vec()
    }

    #[test]
    fn keeps_only_the_unfinished_end() {
        assert_eq!(tail(b"hi \x1b[1;3"), b"\x1b[1;3");
        assert_eq!(tail(b"\x1b[1;31mred"), b"");
        assert_eq!(tail(b"\x1b]0;tit"), b"\x1b]0;tit");
        assert_eq!(tail(b"\x1b]0;title\x07"), b"");
        assert_eq!(tail(b"\x1b]0;title\x1b"), b"\x1b]0;title\x1b");
        assert_eq!(tail(b"\x1b]0;title\x1b\\"), b"");
        assert_eq!(tail(b"\x1b(B"), b"");
        assert_eq!(tail(b"\x1b("), b"\x1b(");
        assert_eq!(tail("é".as_bytes()), b"");
        assert_eq!(tail(&"é".as_bytes()[..1]), &"é".as_bytes()[..1]);
        assert_eq!(tail(&"😀".as_bytes()[..3]), &"😀".as_bytes()[..3]);
    }

    #[test]
    fn controls_inside_a_sequence_are_left_out() {
        // The parser runs the newline at once; replaying it would run it twice.
        assert_eq!(tail(b"\x1b[1\n;2"), b"\x1b[1;2");
    }
}
