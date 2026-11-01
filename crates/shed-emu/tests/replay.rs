//! Replay fidelity: a fresh emulator fed `replay()` must match the original,
//! and keep matching as the same output continues to arrive.

use shed_emu::{AlacrittyEmulator, Emulator, GridSize, Palette, Snapshot};

const COLS: u16 = 40;
const ROWS: u16 = 12;

fn emu() -> AlacrittyEmulator {
    AlacrittyEmulator::new(GridSize::new(COLS, ROWS), Palette::default())
}

fn snapshot(e: &AlacrittyEmulator) -> Snapshot {
    let mut s = Snapshot::default();
    e.snapshot(&Palette::default(), &mut s);
    s
}

/// Compares two emulators page by page through their whole scrollback.
fn assert_same(a: &mut AlacrittyEmulator, b: &mut AlacrittyEmulator, what: &str) {
    assert_eq!(a.modes(), b.modes(), "{what}: modes");
    a.scroll_display(i32::MAX);
    b.scroll_display(i32::MAX);
    assert_eq!(a.display_offset(), b.display_offset(), "{what}: scrollback length");
    loop {
        let offset = a.display_offset();
        let (sa, sb) = (snapshot(a), snapshot(b));
        if let Some(i) = (0..sa.cells.len()).find(|&i| sa.cells[i] != sb.cells[i]) {
            panic!(
                "{what}: grids differ at scroll offset {offset}, row {} col {}\n  original: {:?}\n  replayed: {:?}\n--- original\n{}--- replayed\n{}",
                i / sa.cols,
                i % sa.cols,
                sa.cells[i],
                sb.cells[i],
                a.dump_text(),
                b.dump_text()
            );
        }
        assert_eq!(sa.cursor, sb.cursor, "{what}: cursor at offset {offset}");
        if offset == 0 {
            break;
        }
        a.scroll_display(-1);
        b.scroll_display(-1);
    }
}

/// Feeds `before`, restores a copy from the replay, then feeds `after` to both.
fn check(name: &str, before: &[u8], after: &[u8]) {
    let mut original = emu();
    original.advance(before);
    let replay = original.replay();

    let mut restored = emu();
    restored.advance(&replay);
    assert!(restored.take_replies().is_empty(), "{name}: the replay itself must not trigger replies");
    original.take_replies();
    assert_same(&mut original, &mut restored, &format!("{name}, restored"));
    assert_eq!(restored.replay(), replay, "{name}: replaying the replay changes it");

    original.advance(after);
    restored.advance(after);
    assert_same(&mut original, &mut restored, &format!("{name}, after more output"));
    assert_eq!(original.take_replies(), restored.take_replies(), "{name}: replies to later queries");
}

#[test]
fn shell_session_with_scrollback() {
    let mut before = Vec::new();
    for i in 0..300 {
        before.extend(format!("\x1b[1;32muser@host\x1b[0m:\x1b[34m~/dir\x1b[0m$ ls {i}\r\n").bytes());
        before.extend(format!("file-{i}\tsecond\t\x1b[38;5;208mthird\x1b[0m 日本\r\n").bytes());
    }
    before.extend(b"a very long line that will certainly wrap past the forty column edge of this tiny screen\r\n");
    before.extend("e\u{301}\u{302} combining, \x1b]8;id=x;https://example.com\x1b\\link\x1b]8;;\x1b\\ done\r\n".bytes());
    before.extend(b"\x1b[44m bg to the end \x1b[K\x1b[0m\r\n");
    before.extend(b"\x1b[4:3;58;2;255;0;0mcurly\x1b[0m \x1b[9;2;3mstruck\x1b[0m \x1b[7;8mhidden\x1b[0m\r\n");
    before.extend(b"$ half typed \x1b[1;31m");
    check("shell", &before, b"continues in bold red\r\n\x1b[0mand plain\r\n");
}

#[test]
fn wide_char_at_wrap_edge() {
    let mut before = "x".repeat(COLS as usize - 1).into_bytes();
    before.extend("日本語".bytes());
    check("wide edge", &before, "続き".as_bytes());
}

#[test]
fn pending_wrap_at_bottom_right() {
    let mut before = b"\x1b[12;1H".to_vec();
    before.extend("y".repeat(COLS as usize).bytes());
    check("pending wrap", &before, b"Z after wrap");
}

#[test]
fn alt_screen_keeps_primary_and_modes() {
    let mut before = Vec::new();
    for i in 0..50 {
        before.extend(format!("shell line {i}\r\n").bytes());
    }
    before.extend(b"$ vim\x1b[?1049h\x1b[?1h\x1b=\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[6 q\x1b[?1004h");
    before.extend(b"\x1b[2;11r\x1b[H\x1b[7m status \x1b[0m");
    for row in 2..=11 {
        before.extend(format!("\x1b[{row};1H\x1b[3{}m~ line {row}", row % 8).bytes());
    }
    before.extend(b"\x1b[5;7H\x1b[?25l");
    check(
        "alt",
        &before,
        b"\x1b[11;1H\n\nscrolled in region\x1b[?25h\x1b[r\x1b[?1049l$ back in the shell\r\n",
    );
}

#[test]
fn saved_cursor_charsets_origin_and_tabs() {
    check("decsc", b"\x1b[5;10H\x1b[1;32m\x1b7\x1b[H\x1b[0mtop", b"\x1b8X");
    check("charset", b"\x1b(0lqqk\r\n", b"x  x\r\nmqqj");
    check("so/si", b"\x1b)0\x0eqqq", b"q\x0fq");
    check("origin", b"\x1b[5;15r\x1b[?6h\x1b[3;4H", b"A\x1b[1;1HB\n\n\n\n\n\n\n\n\n\n\n\nC");
    check("tabs", b"\x1b[3g\x1b[1;5H\x1bH\x1b[1;13H\x1bH\x1b[H", b"\r\tX\tY");
    check("insert/autowrap", b"abc\x1b[4h\x1b[?7l\x1b[1;2H", b"ZZ\x1b[1;35H0123456789");
}

#[test]
fn colours_title_and_kitty_keyboard() {
    check(
        "colours",
        b"\x1b]4;1;rgb:12/34/56\x1b\\\x1b]11;rgb:01/02/03\x1b\\\x1b]2;my title\x07\x1b[>5u\x1b[31mred",
        b" more",
    );
}

#[test]
fn synchronized_update_in_progress_is_kept() {
    let mut original = emu();
    original.advance(b"before\r\n\x1b[?2026hinside sync");
    let replay = original.replay();
    let mut restored = emu();
    restored.advance(&replay);
    assert!(restored.dump_text().contains("inside sync"));
}

/// Real program output recorded with `script`, cut at many points.
#[test]
fn recorded_sessions_cut_anywhere() {
    for name in ["vi.raw", "less.raw", "ls.raw"] {
        let path = format!("{}/tests/recordings/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        // Cut where a control sequence starts, as a PTY read boundary usually does.
        let cuts: Vec<usize> = (1..bytes.len()).filter(|&i| bytes[i] == 0x1b).step_by(7).collect();
        assert!(!cuts.is_empty(), "{name} has no escape sequences");
        for cut in cuts {
            check(&format!("{name} cut at {cut}"), &bytes[..cut], &bytes[cut..]);
        }
    }
}
