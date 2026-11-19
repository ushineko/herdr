//! Session persistence — save/restore workspaces, layouts, and working directories.
//!
//! Stored at `~/.config/herdr/session.json`.
//! Optional pane screen history is stored separately at `session-history.json`.
//! Installed plugins are persisted separately at `plugins.json`.

mod io;
pub mod plugin_registry;
mod restore;
mod snapshot;
mod stack_restore;
mod writer;

pub use self::io::{clear_history, load, load_history};
pub use self::restore::restore;
pub(crate) use self::restore::restored_worktree_space_membership;
#[cfg(unix)]
pub use self::restore::{handoff_pane_aliases, restore_handoff};
pub use self::snapshot::{
    capture, capture_history, DirectionSnapshot, DockPaneSnapshot, DockSnapshot, LayoutSnapshot,
    SessionHistorySnapshot, SessionSnapshot, StackSnapshot, TabSnapshot, WorkspaceSnapshot,
};

/// Local patch: dock layouts use the same layout tree format as tabs.
pub(crate) fn capture_layout(node: &crate::layout::Node) -> LayoutSnapshot {
    snapshot::capture_node(node)
}

/// Rebuild a saved layout tree with fresh pane ids, returning the old-to-new map.
pub(crate) fn restore_layout(
    snap: &LayoutSnapshot,
) -> (
    crate::layout::Node,
    std::collections::HashMap<u32, crate::layout::PaneId>,
) {
    restore::restore_node_remapped(snap)
}
pub(crate) use self::writer::SessionWriter;
