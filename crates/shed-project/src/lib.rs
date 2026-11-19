//! A project's files, without UI: the languages Shed colours, the file list
//! (honouring `.gitignore` and the tab's own excludes), a folder's entries,
//! following the disk as it changes, ranking files by a typed name, pictures
//! as data, and unsaved edits kept aside.

pub mod files;
pub mod folder;
pub mod fuzzy;
pub mod lang;
pub mod picture;
pub mod recovery;
pub mod search;
pub mod watch;

pub use files::{Excluder, walk};
pub use folder::{FolderEntry, project_holding, project_root, read_folder};
pub use lang::{Lang, Spans, Style, by_name, detect, highlight};
pub use watch::Watch;
