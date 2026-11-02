//! A program's output as plain text: escape sequences and control
//! characters taken out, across chunk boundaries. For looking for words in
//! output (a dev server's "ready in"), not for display.

#[derive(Default)]
pub struct Plain {
    state: State,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    /// OSC, DCS and the like, until BEL or ST.
    Str,
    StrEscape,
}

impl Plain {
    /// Appends the text in `bytes` to `out`, lines kept, everything else dropped.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            self.state = match (self.state, b) {
                (State::Ground, 0x1b) => State::Escape,
                (State::Ground, b'\n') => {
                    out.push(b'\n');
                    State::Ground
                }
                (State::Ground, b) if b < 0x20 || b == 0x7f => State::Ground,
                (State::Ground, b) => {
                    out.push(b);
                    State::Ground
                }
                (State::Escape, b'[') => State::Csi,
                (State::Escape, b']' | b'P' | b'_' | b'^' | b'X') => State::Str,
                (State::Escape, 0x20..=0x2f) => State::Escape,
                (State::Escape, _) => State::Ground,
                (State::Csi, 0x40..=0x7e) => State::Ground,
                (State::Csi, _) => State::Csi,
                (State::Str, 0x07) => State::Ground,
                (State::Str, 0x1b) => State::StrEscape,
                (State::Str, _) => State::Str,
                (State::StrEscape, b'\\') => State::Ground,
                (State::StrEscape, _) => State::Str,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_titles_go() {
        let mut p = Plain::default();
        let mut out = Vec::new();
        p.feed(b"\x1b]0;title\x07\x1b[1;32m  VITE v5\x1b[0m ready in \x1b[1", &mut out);
        p.feed(b"m412\x1b[22m ms\r\n", &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "  VITE v5 ready in 412 ms\n");
    }
}
