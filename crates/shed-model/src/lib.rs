//! The bench → tab → pane tree, stable IDs, colours and tags.
//!
//! Every crate shares these types: the keeper holds the authoritative tree,
//! windows render it, and saved benches serialize it.

mod color;
pub mod config;
mod ids;
pub mod paths;
mod tree;

pub use color::{BenchColor, Rgb, TAG_COLORS, default_tag_color, tag_color};
pub use ids::{BenchId, PaneId, TabId};
pub use tree::{AiState, Aliases, Bench, Held, Direction, Pane, PaneNode, PaneSetup, Rect, Shed, SplitDir, Tab, Tag, Tool, WindowPlace, WindowState, slug, split_rect};
