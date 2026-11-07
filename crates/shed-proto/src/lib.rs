//! Messages between windows (`shed`) and the keeper (`shedd`), and their framing.
//!
//! Every frame is `[len: u32 LE][kind: u8][payload]`, where `len` counts the
//! kind byte and the payload. Control messages are `postcard`-encoded.
//! Terminal bytes (`Input`, `Output`) travel raw after a 16-byte pane ID, so
//! output is never re-encoded on its way from the PTY to a window.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use shed_model::{Aliases, Bench, BenchColor, BenchId, PaneId, PaneNode, PaneSetup, Shed, SplitDir, TabId, Tag, WindowPlace};

/// Bumped on any incompatible change; a window refuses a keeper with another version.
pub const PROTOCOL_VERSION: u32 = 18;

/// Frames larger than this are a protocol error, not a reason to allocate.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

const KIND_CLIENT: u8 = 0;
const KIND_KEEPER: u8 = 1;
const KIND_INPUT: u8 = 2;
const KIND_OUTPUT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKind {
    /// A `shed` process drawing bench windows.
    Window,
    /// A one-shot CLI command such as `shed status`.
    Cli,
}

/// Sent by a window or CLI client to the keeper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message; may be sent again to change the client's kind.
    Hello { version: u32, kind: ClientKind },
    /// Start receiving a bench's output. `None` attaches to the first bench,
    /// creating one if the keeper holds none.
    Attach { bench: Option<BenchId> },
    /// Stop receiving a bench's output; its tools keep running.
    Detach { bench: BenchId },
    /// The pane's new grid size, plus the cell size in pixels for apps that ask.
    Resize { pane: PaneId, cols: u16, rows: u16, cell_width: u16, cell_height: u16 },
    /// Ask for a pane's current screen again (see [`KeeperMsg::Replay`]).
    Replay { pane: PaneId },
    /// Show benches in windows. The keeper passes this on to the window
    /// process as [`KeeperMsg::Show`], so a second `shed` reuses the first.
    Show(ShowTarget),
    NewTab { bench: BenchId, cwd: Option<PathBuf> },
    /// Splits `pane`; the new pane opens in `cwd`, or the bench's directory.
    Split { pane: PaneId, dir: SplitDir, cwd: Option<PathBuf> },
    /// Hangs up the pane's shell; the pane leaves the tree when it exits.
    ClosePane { pane: PaneId },
    CloseTab { tab: TabId },
    /// Saves the bench, then stops every tool on it. It stays saved, to load later.
    CloseBench { bench: BenchId },
    /// Stops the bench if it runs, and removes its saved file and launcher.
    DeleteBench { name: String },
    /// A pane's startup and teardown; used the next time its bench loads or closes.
    SetPaneSetup { pane: PaneId, setup: PaneSetup },
    /// Replaces the Shed aliases at one level.
    SetAliases { level: AliasTarget, aliases: Aliases },
    /// Saves a bench now; with `as_name`, also saves a copy under that name.
    SaveBench { bench: BenchId, as_name: Option<String> },
    FocusPane { pane: PaneId },
    FocusTab { tab: TabId },
    /// New split ratios for a tab. The panes must be the same.
    SetLayout { tab: TabId, layout: PaneNode },
    RenameBench { bench: BenchId, name: String },
    RenameTab { tab: TabId, name: Option<String> },
    SetBenchColor { bench: BenchId, color: BenchColor },
    SetBadge { bench: BenchId, badge: Option<String> },
    SetTags { tab: TabId, tags: Vec<Tag> },
    SetPrivate { tab: TabId, private: bool },
    /// Moves a tab within its bench or to another bench, at `index`.
    MoveTab { tab: TabId, bench: BenchId, index: usize },
    Status,
    /// Stop the keeper and every tool it holds.
    Shutdown,
    // New messages go last: variants travel by position, and `shed stop`
    // from a newer client has to reach an older keeper.
    /// Asks for [`KeeperMsg::Pegboard`].
    Pegboard,
    /// Closes Shed: records which benches run (and which have windows) for
    /// `auto_restore`, closes each bench as `CloseBench` does, then stops.
    QuitShed,
    /// The desktop prefers dark or light, for `theme = "auto"`: the keeper
    /// answers apps' colour queries from the theme in effect.
    SetAppearance { dark: bool },
    /// Changes a closed bench's saved file.
    EditSaved { name: String, edit: SavedEdit },
    /// Moves a tab where either bench is closed (both running: `MoveTab`).
    /// A running tab moved into a closed bench stops, as closing would stop it.
    MoveTabAcross { tab: TabId, from: BenchRef, to: BenchRef, index: usize },
    /// Stops a pane's AI request.
    CancelAsk { pane: PaneId },
    /// A tab's zoom: points over the configured font size.
    SetZoom { tab: TabId, zoom: f32 },
    /// The order benches are listed in, running and closed.
    SetBenchOrder(Vec<BenchId>),
    /// Starts a pane's held startup without waiting for the panes it waits for.
    StartNow { pane: PaneId },
    /// Where a bench's window is (or, without a bench, the Shed window's), to
    /// open it there again.
    SetWindow { bench: Option<BenchId>, place: WindowPlace },
    /// Closes a bench's window; it keeps running.
    Hide { bench: BenchId },
    /// Sets (or, with no value, removes) one of the shell's own aliases, in
    /// the file its startup files source.
    SetShellAlias { name: String, value: Option<String> },
}

/// What can change in a closed bench without opening it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SavedEdit {
    Rename(String),
    Color(BenchColor),
    Badge(Option<String>),
    RenameTab { tab: TabId, name: Option<String> },
    TabTags { tab: TabId, tags: Vec<Tag> },
    TabPrivate { tab: TabId, private: bool },
    /// Adds a tab with one pane in the bench's folder.
    NewTab,
    RemoveTab { tab: TabId },
    MoveTab { tab: TabId, index: usize },
    PaneSetup { pane: PaneId, setup: PaneSetup },
    /// The bench's aliases (`tab: None`) or a tab's.
    Aliases { tab: Option<TabId>, aliases: Aliases },
}

/// A bench, running or closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BenchRef {
    Running(BenchId),
    /// A closed bench, by name.
    Saved(String),
}

/// Which aliases to set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AliasTarget {
    Global,
    Bench(BenchId),
    Tab(TabId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShowTarget {
    /// Every running bench without a window.
    Detached,
    Bench(BenchId),
    NewBench { name: Option<String>, cwd: Option<PathBuf> },
    /// A bench by name: shown if it runs, loaded from its saved file if not.
    Load { name: String },
    /// The Shed window. With `auto_restore`, a keeper running nothing first
    /// brings back the benches of the last session.
    Shed,
}

/// Sent by the keeper to a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeeperMsg {
    Welcome { version: u32, keeper_pid: u32 },
    /// The whole tree, sent on attach and after every change.
    TreeChanged(Shed),
    Attached { bench: BenchId },
    /// A pane's whole state (scrollback, screens, modes) as terminal bytes:
    /// fed to a fresh emulator of `cols` × `rows`, it shows the pane exactly
    /// as the keeper sees it. Sent for every pane of a bench on attach, before
    /// any of that pane's live output, and for each new pane.
    Replay { pane: PaneId, cols: u16, rows: u16, bytes: Vec<u8> },
    /// To the window process: open (or raise) this bench's window.
    Show { bench: BenchId },
    PaneExited { pane: PaneId, code: Option<i32> },
    Status(KeeperStatus),
    /// A request finished; the text says what happened (for the CLI).
    Done(String),
    Error(String),
    // New messages go last, so an older keeper's errors still read as errors.
    Pegboard(PegboardInfo),
    /// To the window process: open (or raise) the Shed window.
    ShowShed,
    /// To the window process: close this bench's window, keeping the bench.
    Hide { bench: BenchId },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeeperStatus {
    pub pid: u32,
    pub version: String,
    pub uptime_secs: u64,
    pub clients: u32,
    /// Window processes connected.
    pub windows: u32,
    /// Benches some window shows; the rest are detached.
    pub open: Vec<BenchId>,
    /// Saved benches that are not running.
    pub stored: Vec<String>,
    pub tree: Shed,
}

/// What the pegboard shows beyond the tree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PegboardInfo {
    /// Saved benches that are not running, as they would load.
    pub stored: Vec<StoredBench>,
    /// The last lines with text on screen, of the active pane of every tab,
    /// running or saved.
    pub previews: Vec<(PaneId, Vec<String>)>,
    /// The order benches are listed in, as the user arranged them; others
    /// follow by name.
    pub order: Vec<BenchId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredBench {
    pub bench: Bench,
    /// When it was saved, UTC.
    pub saved: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Client(ClientMsg),
    Keeper(KeeperMsg),
    /// Keystrokes and pastes going to a pane's PTY.
    Input { pane: PaneId, bytes: Vec<u8> },
    /// Bytes a pane's PTY produced.
    Output { pane: PaneId, bytes: Vec<u8> },
}

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    #[error("truncated frame")]
    Truncated,
    #[error("bad control message: {0}")]
    Decode(#[from] postcard::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Frame {
    /// Appends the encoded frame to `out`.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&[0; 4]);
        match self {
            Frame::Client(msg) => {
                out.push(KIND_CLIENT);
                postcard::to_io(msg, &mut *out).expect("writing to a Vec cannot fail");
            }
            Frame::Keeper(msg) => {
                out.push(KIND_KEEPER);
                postcard::to_io(msg, &mut *out).expect("writing to a Vec cannot fail");
            }
            Frame::Input { pane, bytes } | Frame::Output { pane, bytes } => {
                out.push(if matches!(self, Frame::Input { .. }) { KIND_INPUT } else { KIND_OUTPUT });
                out.extend_from_slice(pane.as_bytes());
                out.extend_from_slice(bytes);
            }
        }
        let len = (out.len() - start - 4) as u32;
        out[start..start + 4].copy_from_slice(&len.to_le_bytes());
    }

    /// Encodes an `Output` frame straight from a borrowed buffer, without an
    /// intermediate copy. This is the keeper's hot path.
    pub fn encode_output(pane: PaneId, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 1 + 16 + bytes.len());
        out.extend_from_slice(&((1 + 16 + bytes.len()) as u32).to_le_bytes());
        out.push(KIND_OUTPUT);
        out.extend_from_slice(pane.as_bytes());
        out.extend_from_slice(bytes);
        out
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    /// Decodes one frame body (kind byte and payload, without the length prefix).
    pub fn decode_body(body: &[u8]) -> Result<Frame, ProtoError> {
        let (&kind, payload) = body.split_first().ok_or(ProtoError::Truncated)?;
        Ok(match kind {
            KIND_CLIENT => Frame::Client(postcard::from_bytes(payload)?),
            KIND_KEEPER => Frame::Keeper(postcard::from_bytes(payload)?),
            KIND_INPUT | KIND_OUTPUT => {
                if payload.len() < 16 {
                    return Err(ProtoError::Truncated);
                }
                let (id, bytes) = payload.split_at(16);
                let pane = PaneId::from_bytes(id.try_into().unwrap());
                let bytes = bytes.to_vec();
                if kind == KIND_INPUT {
                    Frame::Input { pane, bytes }
                } else {
                    Frame::Output { pane, bytes }
                }
            }
            other => return Err(ProtoError::UnknownKind(other)),
        })
    }
}

/// Incremental decoder for a byte stream of frames.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    pos: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, or `None` until more bytes arrive.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtoError> {
        let avail = &self.buf[self.pos..];
        if avail.len() < 4 {
            self.compact();
            return Ok(None);
        }
        let len = u32::from_le_bytes(avail[..4].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return Err(ProtoError::TooLarge(len));
        }
        if avail.len() < 4 + len {
            self.compact();
            return Ok(None);
        }
        let frame = Frame::decode_body(&avail[4..4 + len])?;
        self.pos += 4 + len;
        Ok(Some(frame))
    }

    fn compact(&mut self) {
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }
}

/// Blocking read of one frame.
pub fn read_frame(r: &mut impl Read) -> Result<Frame, ProtoError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(ProtoError::TooLarge(len));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    Frame::decode_body(&body)
}

/// Blocking write of one frame.
pub fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&frame.encode())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Messages travel by variant position. These three must keep theirs, so
    /// any `shed stop` can stop any keeper and a version mismatch is reported
    /// as an error; new variants go at the end.
    #[test]
    fn versioning_messages_keep_their_positions() {
        let first = |bytes: Vec<u8>| bytes[0];
        assert_eq!(first(postcard::to_allocvec(&ClientMsg::Hello { version: 1, kind: ClientKind::Cli }).unwrap()), 0);
        assert_eq!(first(postcard::to_allocvec(&ClientMsg::Shutdown).unwrap()), 26);
        assert_eq!(first(postcard::to_allocvec(&KeeperMsg::Error(String::new())).unwrap()), 8);
    }

    #[test]
    fn round_trip_through_decoder_byte_by_byte() {
        let pane = PaneId::new();
        let frames = vec![
            Frame::Client(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window }),
            Frame::Input { pane, bytes: b"ls -la\r".to_vec() },
            Frame::Output { pane, bytes: vec![0x1b, b'[', b'm', 0xff, 0] },
            Frame::Keeper(KeeperMsg::PaneExited { pane, code: Some(0) }),
            Frame::Output { pane, bytes: Vec::new() },
        ];
        let mut wire = Vec::new();
        for f in &frames {
            f.encode_into(&mut wire);
        }
        let mut dec = FrameDecoder::new();
        let mut got = Vec::new();
        for b in wire {
            dec.push(&[b]);
            while let Some(f) = dec.next_frame().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got, frames);
    }

    #[test]
    fn encode_output_matches_encode() {
        let pane = PaneId::new();
        let bytes = b"\x1b[1mhi".to_vec();
        assert_eq!(Frame::encode_output(pane, &bytes), Frame::Output { pane, bytes }.encode());
    }

    #[test]
    fn blocking_read_write() {
        let frame = Frame::Keeper(KeeperMsg::Error("nope".into()));
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(read_frame(&mut wire.as_slice()).unwrap(), frame);
    }

    #[test]
    fn rejects_huge_frames() {
        let mut dec = FrameDecoder::new();
        dec.push(&u32::MAX.to_le_bytes());
        assert!(matches!(dec.next_frame(), Err(ProtoError::TooLarge(_))));
    }
}
