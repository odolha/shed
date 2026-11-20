//! What the running link (`shed link`) says about itself, in a file in the
//! runtime folder: for `shed nexus status` and the Shed window.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// How the link is doing, as it last wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkStatus {
    pub pid: u32,
    /// `connecting`, `connected`, `offline` or `refused`.
    pub state: String,
    /// The workspace's name at the nexus, once welcomed.
    #[serde(default)]
    pub name: Option<String>,
    /// Why it is not connected, if it says.
    #[serde(default)]
    pub error: Option<String>,
    /// Since when, Unix seconds.
    pub since: u64,
}

impl LinkStatus {
    pub fn write(&self, path: &Path) {
        let _ = shed_model::paths::write_whole(path, serde_json::to_string(self).expect("serializes"));
    }

    /// The running link's status; `None` when no link runs.
    pub fn read(path: &Path) -> Option<LinkStatus> {
        let s: LinkStatus = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        crate::keeper_conn::alive(s.pid).then_some(s)
    }

    /// The status of the link running for this keeper, if one runs.
    pub fn current() -> Option<LinkStatus> {
        Self::read(&shed_model::paths::link_status_path())
    }
}
