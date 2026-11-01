//! Messages between windows (`shed`) and the keeper (`shedd`), and their framing.
//!
//! Every frame is `[len: u32 LE][kind: u8][payload]`, where `len` counts the
//! kind byte and the payload. Control messages are `postcard`-encoded.
//! Terminal bytes (`Input`, `Output`) travel raw after a 16-byte pane ID, so
//! output is never re-encoded on its way from the PTY to a window.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use shed_model::{BenchId, PaneId, Shed};

/// Bumped on any incompatible change; a window refuses a keeper with another version.
pub const PROTOCOL_VERSION: u32 = 1;

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
    Hello { version: u32, kind: ClientKind },
    /// Start receiving a bench's output. `None` attaches to the first bench,
    /// creating one if the keeper holds none.
    Attach { bench: Option<BenchId> },
    /// Stop receiving a bench's output; its tools keep running.
    Detach { bench: BenchId },
    /// The pane's new grid size, plus the cell size in pixels for apps that ask.
    Resize { pane: PaneId, cols: u16, rows: u16, cell_width: u16, cell_height: u16 },
    NewBench { name: Option<String>, cwd: Option<PathBuf> },
    Status,
    /// Stop the keeper and every tool it holds.
    Shutdown,
}

/// Sent by the keeper to a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeeperMsg {
    Welcome { version: u32, keeper_pid: u32 },
    /// The whole tree, sent on attach and after every change.
    TreeChanged(Shed),
    Attached { bench: BenchId },
    PaneExited { pane: PaneId, code: Option<i32> },
    Status(KeeperStatus),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeeperStatus {
    pub pid: u32,
    pub version: String,
    pub uptime_secs: u64,
    pub clients: u32,
    pub tree: Shed,
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
