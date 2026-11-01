//! Telling binary data apart from terminal text.
//!
//! `cat image.png` sends random bytes, and some happen to form queries such
//! as `ESC Z` or `CSI c`. A terminal that answers them types the answers into
//! the shell's input once `cat` ends. Programs that really query the terminal
//! write clean text, so replies are dropped while output looks binary.

use std::time::{Duration, Instant};

/// How long after binary output replies stay suppressed. Long enough to cover
/// the chunks of one `cat`, far shorter than starting another program by hand.
pub const BINARY_WINDOW: Duration = Duration::from_millis(250);

/// True for bytes no text-producing program would write: NUL, or invalid UTF-8.
/// A UTF-8 character split across chunk boundaries does not count.
pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    // Up to three continuation bytes may finish a character from the previous chunk.
    let lead = bytes.iter().take(3).take_while(|b| (0x80..0xC0).contains(*b)).count();
    match std::str::from_utf8(&bytes[lead..]) {
        Ok(_) => false,
        // `error_len() == None`: a character cut off at the end, finished by the next chunk.
        Err(e) => e.error_len().is_some(),
    }
}

/// Decides, chunk by chunk, whether replies to the terminal's queries may be sent.
#[derive(Debug, Default)]
pub struct ReplyGuard {
    last_binary: Option<Instant>,
}

impl ReplyGuard {
    /// Records a chunk of output; returns `true` if replies it triggered must be dropped.
    pub fn suppress(&mut self, bytes: &[u8], now: Instant) -> bool {
        if looks_binary(bytes) {
            self.last_binary = Some(now);
            return true;
        }
        self.last_binary.is_some_and(|t| now.duration_since(t) < BINARY_WINDOW)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AlacrittyEmulator, Emulator, GridSize, Palette};

    #[test]
    fn text_is_not_binary() {
        assert!(!looks_binary(b"\x1b[1mhello\x1b[0m\r\n"));
        assert!(!looks_binary("日本語 ✓".as_bytes()));
        // Split characters at either edge of a chunk.
        let s = "日本".as_bytes();
        assert!(!looks_binary(&s[..4]));
        assert!(!looks_binary(&s[1..]));
    }

    #[test]
    fn binary_is_binary() {
        assert!(looks_binary(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"));
        assert!(looks_binary(b"text \x89PNG"));
        assert!(looks_binary(b"abc\0def"));
        assert!(looks_binary(b"abc\xff\x1bZdef"));
    }

    #[test]
    fn queries_inside_binary_get_no_reply() {
        let mut e = AlacrittyEmulator::new(GridSize::new(80, 24), Palette::default());
        let mut guard = ReplyGuard::default();
        let now = Instant::now();
        // Random bytes that happen to contain DECID (ESC Z) and DA (CSI c).
        let garbage = b"\x89PNG\x00\x13\xfe\x1bZ\xc3\x28\x1b[c\x9a";
        e.advance(garbage);
        assert!(!e.take_replies().is_empty(), "the core answers the queries");
        assert!(guard.suppress(garbage, now));
        // A clean chunk right after is still part of the same burst.
        assert!(guard.suppress(b"\x1bZ", now + Duration::from_millis(10)));
        // A real program querying later is answered.
        assert!(!guard.suppress(b"\x1b[c", now + BINARY_WINDOW));
    }
}
