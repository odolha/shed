//! The bench → tab → pane tree, stable IDs, colours and tags.
//!
//! Every crate shares these types: the keeper holds the authoritative tree,
//! windows render it, and saved benches serialize it.

mod color;
pub mod config;
mod ids;
pub mod links;
pub mod paths;
mod tree;

pub use color::{BenchColor, Rgb, TAG_COLORS, default_tag_color, tag_color};
pub use ids::{BenchId, PaneId, TabId};
pub use tree::{AiActivity, AiState, AiTab, Aliases, Draft, DraftSession, Bench, Held, Direction, Pane, PaneNode, PaneSetup, Project, ProjectLink, Queued, Rect, RemoteTab, Shed, SplitDir, Tab, TabDivider, TabKind, Tag, Tool, WindowPlace, WindowState, slug, split_rect};
