//! A project's files, without UI: the languages Shed colours, the file list
//! (honouring `.gitignore` and the tab's own excludes), following the disk
//! as it changes, and ranking files by a typed name.

pub mod files;
pub mod fuzzy;
pub mod lang;
pub mod watch;

pub use files::{Excluder, walk};
pub use lang::{Lang, Spans, Style, detect, highlight};
pub use watch::Watch;
