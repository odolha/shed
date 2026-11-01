//! The bench → tab → pane tree, stable IDs, colours and tags.
//!
//! Every crate shares these types: the keeper holds the authoritative tree,
//! windows render it, and saved benches serialize it.

mod color;
mod ids;
pub mod paths;
mod tree;

pub use color::{BenchColor, Rgb};
pub use ids::{BenchId, PaneId, TabId};
pub use tree::{Bench, Pane, PaneNode, Shed, SplitDir, Tab, Tag};
