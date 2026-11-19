//! The nexus API: the messages between a workspace's link, the nexus and the
//! connectors (web app, Android app). See `docs/nexus-plan.md`.
//!
//! JSON everywhere. Conventions, so the TypeScript and Kotlin made from these
//! types (`cargo run -p shed-codegen`) read the same JSON:
//!
//! - enums with data are adjacently tagged: `{"type": "Variant", "content": …}`;
//!   enums without data are plain strings;
//! - struct fields are camelCase;
//! - no 64-bit integers (JavaScript numbers): times are Unix seconds in `u32`,
//!   sizes in `u32`, or `f64` where they may be large;
//! - new fields get `#[serde(default)]`, so older peers' messages still read.
//!
//! The nexus relays workspace calls, topics and events without reading them
//! (it parses only the envelope, `wire`), so a link and a connector may know
//! calls the nexus does not.

pub mod calls;
pub mod error;
pub mod http;
pub mod state;
pub mod topics;
pub mod wire;

pub use error::{ApiError, ErrorCode};

/// The API's version. Bumped when a change is not additive; peers say theirs in
/// `Hello` and the nexus accepts `MIN_API_VERSION..=API_VERSION`.
pub const API_VERSION: u32 = 1;
/// The oldest version the nexus still accepts.
pub const MIN_API_VERSION: u32 = 1;
