//! Edits to a bench, running or closed: a running bench changes through the
//! keeper's tree, a closed one in its saved file (`ClientMsg::EditSaved`).
//! Clients say what changes; `BenchEdit::to_msg` picks the message.

use std::path::PathBuf;

use shed_model::{Aliases, BenchColor, PaneId, PaneSetup, ProjectLink, TabId, Tag};
use shed_proto::{AliasTarget, BenchRef, ClientMsg, SavedEdit};

/// What can change about a bench (or a tab or pane of it), whether it runs or not.
#[derive(Clone, Debug, PartialEq)]
pub enum BenchEdit {
    Rename(String),
    Color(BenchColor),
    Badge(Option<String>),
    /// The bench's links, all of them.
    Links(Vec<ProjectLink>),
    /// A tab with one pane in the bench's folder.
    NewTab,
    /// A project tab for the folder `root`.
    NewProjectTab { root: PathBuf },
    /// `None` goes back to the title the shell sets.
    RenameTab { tab: TabId, name: Option<String> },
    TabTags { tab: TabId, tags: Vec<Tag> },
    TabPrivate { tab: TabId, private: bool },
    PaneSetup { pane: PaneId, setup: PaneSetup },
    /// The bench's aliases (`tab: None`) or a tab's.
    Aliases { tab: Option<TabId>, aliases: Aliases },
}

impl BenchEdit {
    /// The message that makes this edit to `bench`.
    pub fn to_msg(self, bench: &BenchRef) -> ClientMsg {
        match bench {
            BenchRef::Running(id) => {
                let bench = *id;
                match self {
                    BenchEdit::Rename(name) => ClientMsg::RenameBench { bench, name },
                    BenchEdit::Color(color) => ClientMsg::SetBenchColor { bench, color },
                    BenchEdit::Badge(badge) => ClientMsg::SetBadge { bench, badge },
                    BenchEdit::Links(links) => ClientMsg::SetBenchLinks { bench, links },
                    BenchEdit::NewTab => ClientMsg::NewTab { bench, cwd: None },
                    BenchEdit::NewProjectTab { root } => ClientMsg::NewProjectTab { bench, root },
                    BenchEdit::RenameTab { tab, name } => ClientMsg::RenameTab { tab, name },
                    BenchEdit::TabTags { tab, tags } => ClientMsg::SetTags { tab, tags },
                    BenchEdit::TabPrivate { tab, private } => ClientMsg::SetPrivate { tab, private },
                    BenchEdit::PaneSetup { pane, setup } => ClientMsg::SetPaneSetup { pane, setup },
                    BenchEdit::Aliases { tab, aliases } => {
                        ClientMsg::SetAliases { level: tab.map_or(AliasTarget::Bench(bench), AliasTarget::Tab), aliases }
                    }
                }
            }
            BenchRef::Saved(name) => {
                let edit = match self {
                    BenchEdit::Rename(name) => SavedEdit::Rename(name),
                    BenchEdit::Color(color) => SavedEdit::Color(color),
                    BenchEdit::Badge(badge) => SavedEdit::Badge(badge),
                    BenchEdit::Links(links) => SavedEdit::Links(links),
                    BenchEdit::NewTab => SavedEdit::NewTab,
                    BenchEdit::NewProjectTab { root } => SavedEdit::NewProjectTab { root },
                    BenchEdit::RenameTab { tab, name } => SavedEdit::RenameTab { tab, name },
                    BenchEdit::TabTags { tab, tags } => SavedEdit::TabTags { tab, tags },
                    BenchEdit::TabPrivate { tab, private } => SavedEdit::TabPrivate { tab, private },
                    BenchEdit::PaneSetup { pane, setup } => SavedEdit::PaneSetup { pane, setup },
                    BenchEdit::Aliases { tab, aliases } => SavedEdit::Aliases { tab, aliases },
                };
                ClientMsg::EditSaved { name: name.clone(), edit }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use shed_model::BenchId;

    use super::*;

    #[test]
    fn running_benches_change_live_closed_ones_in_their_file() {
        let id = BenchId::new();
        let tab = TabId::new();
        let (running, closed) = (BenchRef::Running(id), BenchRef::Saved("api".into()));
        assert_eq!(BenchEdit::NewTab.to_msg(&running), ClientMsg::NewTab { bench: id, cwd: None });
        assert_eq!(BenchEdit::NewTab.to_msg(&closed), ClientMsg::EditSaved { name: "api".into(), edit: SavedEdit::NewTab });
        assert_eq!(
            BenchEdit::Rename("web".into()).to_msg(&closed),
            ClientMsg::EditSaved { name: "api".into(), edit: SavedEdit::Rename("web".into()) }
        );
        assert_eq!(
            BenchEdit::TabPrivate { tab, private: true }.to_msg(&running),
            ClientMsg::SetPrivate { tab, private: true }
        );
        let aliases = Aliases::default();
        assert_eq!(
            BenchEdit::Aliases { tab: None, aliases: aliases.clone() }.to_msg(&running),
            ClientMsg::SetAliases { level: AliasTarget::Bench(id), aliases: aliases.clone() }
        );
        assert_eq!(
            BenchEdit::Aliases { tab: Some(tab), aliases: aliases.clone() }.to_msg(&running),
            ClientMsg::SetAliases { level: AliasTarget::Tab(tab), aliases: aliases.clone() }
        );
        assert_eq!(
            BenchEdit::Aliases { tab: Some(tab), aliases: aliases.clone() }.to_msg(&closed),
            ClientMsg::EditSaved { name: "api".into(), edit: SavedEdit::Aliases { tab: Some(tab), aliases } }
        );
    }
}
