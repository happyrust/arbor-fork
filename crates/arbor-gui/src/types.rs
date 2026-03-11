use {
    serde::{Deserialize, Serialize},
    std::path::PathBuf,
};

/// Identifies a sidebar item for persisted UI ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum SidebarItemId {
    Worktree(PathBuf),
    Outpost(String),
}
