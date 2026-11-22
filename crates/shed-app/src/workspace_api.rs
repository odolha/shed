//! Another workspace's data through the nexus (`shed_client::nexus_api`):
//! what views of a remote workspace use in place of this machine's disk.
//! Here, what they share: ids and paths as the API takes them, positions
//! in UTF-16 units as it gives them, pictures in Base64, errors in words.

use std::path::Path;

use base64::Engine;
use shed_api::ApiError;
use shed_model::TabId;

/// A tab's id as calls name it.
pub fn tab(id: TabId) -> String {
    id.to_string()
}

/// `path` relative to `root`, with `/` between its parts (`""`: the root
/// itself); `None` when it is not inside.
pub fn rel(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

/// Bytes sent as Base64.
pub fn decode(data: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).ok()
}

/// Bytes to send as Base64.
pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A position in UTF-16 code units in `text`, as a byte offset (inside a
/// character: where it starts; past the end: the end).
pub fn byte_of_utf16(text: &str, unit: u32) -> usize {
    let mut units = 0u32;
    for (byte, c) in text.char_indices() {
        if units >= unit {
            return byte;
        }
        units += c.len_utf16() as u32;
        if units > unit {
            return byte;
        }
    }
    text.len()
}

/// A position in UTF-16 code units in `text`, as a character index.
pub fn char_of_utf16(text: &str, unit: u32) -> usize {
    text[..byte_of_utf16(text, unit)].chars().count()
}

/// What went wrong with a call, for people.
pub fn says(e: &ApiError) -> String {
    if e.message.is_empty() { format!("{:?}", e.code).to_lowercase() } else { e.message.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_positions_as_bytes_and_characters() {
        let text = "a🦀b€c";
        // a=0, 🦀=1..3, b=3, €=4, c=5, end=6.
        assert_eq!([0, 1, 3, 4, 5, 6].map(|u| byte_of_utf16(text, u)), [0, 1, 5, 6, 9, 10]);
        assert_eq!(byte_of_utf16(text, 2), 1);
        assert_eq!(byte_of_utf16(text, 99), text.len());
        assert_eq!([0, 1, 3, 4, 5, 6].map(|u| char_of_utf16(text, u)), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn paths_relative_to_a_root() {
        let root = Path::new("/srv/app");
        assert_eq!(rel(root, Path::new("/srv/app/src/main.rs")).as_deref(), Some("src/main.rs"));
        assert_eq!(rel(root, root).as_deref(), Some(""));
        assert_eq!(rel(root, Path::new("/srv/other")), None);
        assert_eq!(decode(&encode(b"\x00png")).unwrap(), b"\x00png");
    }
}
