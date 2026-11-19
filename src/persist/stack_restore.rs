//! Local patch: restoring pane stacks (spec 002, `ushineko/specs/002-nested-tabs.md`).
//!
//! Restore is lenient. A saved stack is rebuilt from its members that the
//! saved tree and pane map still know: exactly one member must be a tree leaf
//! (the visible one); others must have a saved pane entry. Anything else is
//! dropped and logged, never a failed restore.

use std::collections::{HashMap, HashSet};

use tracing::warn;

use super::snapshot::TabSnapshot;
use crate::layout::{Node, PaneId};
use crate::workspace::PaneStack;

/// A saved stack after validation: members by saved id in strip order, and
/// the saved id of the member that is a tree leaf.
pub(super) struct StackPlan {
    members: Vec<u32>,
    visible: u32,
}

/// Validate the saved stacks against the saved tree (`id_map` holds the tree's
/// saved ids) and pane map. A member claimed by an earlier stack, a second
/// tree leaf, or a hidden member without a saved pane entry is dropped; a
/// stack without a leaf, or left with fewer than two members, is dropped.
pub(super) fn plan_stacks(snap: &TabSnapshot, id_map: &HashMap<u32, PaneId>) -> Vec<StackPlan> {
    let mut claimed = HashSet::new();
    let mut plans = Vec::new();
    for (index, stack) in snap.stacks.iter().enumerate() {
        let mut members = Vec::new();
        let mut visible = None;
        for old_id in &stack.panes {
            if claimed.contains(old_id) || members.contains(old_id) {
                continue;
            }
            if id_map.contains_key(old_id) {
                if visible.is_none() {
                    visible = Some(*old_id);
                    members.push(*old_id);
                }
            } else if snap.panes.contains_key(old_id) {
                members.push(*old_id);
            }
        }
        let dropped = stack.panes.len() - members.len();
        match visible {
            Some(visible) if members.len() >= 2 => {
                if dropped > 0 {
                    warn!(
                        stack = index,
                        dropped, "restore: repaired a saved pane stack"
                    );
                }
                claimed.extend(members.iter().copied());
                plans.push(StackPlan { members, visible });
            }
            _ => warn!(
                stack = index,
                members = stack.panes.len(),
                "restore: dropped a saved pane stack without a slot or members"
            ),
        }
    }
    plans
}

/// Fresh pane ids for the hidden members, added to `id_map`. Returned in plan
/// order, so restore spawns them deterministically.
pub(super) fn allocate_hidden_ids(
    plans: &[StackPlan],
    id_map: &mut HashMap<u32, PaneId>,
) -> Vec<PaneId> {
    let mut hidden = Vec::new();
    for plan in plans {
        for old_id in &plan.members {
            if *old_id != plan.visible {
                let id = PaneId::alloc();
                id_map.insert(*old_id, id);
                hidden.push(id);
            }
        }
    }
    hidden
}

/// Build the restored stacks from the members that came back. When the
/// visible member did not, the first surviving hidden member takes its leaf in
/// `node`, before the caller prunes missing leaves.
pub(super) fn finish_stacks(
    plans: &[StackPlan],
    id_map: &HashMap<u32, PaneId>,
    surviving: &HashSet<PaneId>,
    node: &mut Node,
) -> Vec<PaneStack> {
    let mut stacks = Vec::new();
    for plan in plans {
        let Some(&leaf) = id_map.get(&plan.visible) else {
            continue;
        };
        let members: Vec<PaneId> = plan
            .members
            .iter()
            .filter_map(|old_id| id_map.get(old_id).copied())
            .filter(|id| surviving.contains(id))
            .collect();
        let visible = if surviving.contains(&leaf) {
            leaf
        } else {
            let Some(&heir) = members.first() else {
                continue;
            };
            replace_leaf(node, leaf, heir);
            heir
        };
        if members.len() < 2 {
            continue;
        }
        let Some(active) = members.iter().position(|id| *id == visible) else {
            continue;
        };
        stacks.push(PaneStack {
            panes: members,
            active,
        });
    }
    stacks
}

fn replace_leaf(node: &mut Node, old: PaneId, new: PaneId) -> bool {
    match node {
        Node::Pane(id) if *id == old => {
            *id = new;
            true
        }
        Node::Pane(_) => false,
        Node::Split { first, second, .. } => {
            replace_leaf(first, old, new) || replace_leaf(second, old, new)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::sync::{mpsc, Notify};

    use super::super::snapshot::{
        DirectionSnapshot, LayoutSnapshot, PaneSnapshot, SessionSnapshot, StackSnapshot,
        TabSnapshot, WorkspaceSnapshot, SNAPSHOT_VERSION,
    };
    use super::*;
    use crate::workspace::Workspace;

    fn pane(cwd: &str) -> PaneSnapshot {
        PaneSnapshot {
            cwd: PathBuf::from(cwd),
            label: None,
            agent_name: None,
            managed_agent_kind: None,
            agent_session: None,
            agent_resume: None,
            launch_argv: None,
        }
    }

    /// `[10 | 20]`, with 30 and 40 saved as hidden members of 20's stack.
    fn stacked_session(stacks: Vec<StackSnapshot>, root: u32, focused: u32) -> SessionSnapshot {
        SessionSnapshot {
            version: SNAPSHOT_VERSION,
            workspaces: vec![WorkspaceSnapshot {
                id: Some("w1".into()),
                custom_name: None,
                identity_cwd: PathBuf::from("/"),
                worktree_space: None,
                public_pane_numbers: HashMap::from([(10, 1), (20, 2), (30, 5), (40, 7)]),
                next_public_pane_number: 8,
                public_tab_numbers: vec![1],
                next_public_tab_number: 2,
                tabs: vec![TabSnapshot {
                    custom_name: None,
                    layout: LayoutSnapshot::Split {
                        direction: DirectionSnapshot::Horizontal,
                        ratio: 0.3,
                        first: Box::new(LayoutSnapshot::Pane(10)),
                        second: Box::new(LayoutSnapshot::Pane(20)),
                    },
                    panes: HashMap::from([
                        (10, pane("/")),
                        (20, pane("/tmp")),
                        (30, pane("/usr")),
                        (40, pane("/etc")),
                    ]),
                    zoomed: false,
                    focused: Some(focused),
                    root_pane: Some(root),
                    stacks,
                }],
                active_tab: 0,
            }],
            active: Some(0),
            selected: 0,
            sidebar_width: None,
            sidebar_section_split: None,
            collapsed_space_keys: Default::default(),
            dock: None,
        }
    }

    fn restore(snapshot: &SessionSnapshot) -> (Workspace, HashMap<PaneId, PathBuf>) {
        let (events, _rx) = mpsc::channel(16);
        let (mut workspaces, terminals, _runtimes) = super::super::restore(
            snapshot,
            None,
            24,
            80,
            0,
            "/bin/sh",
            crate::config::ShellModeConfig::NonLogin,
            false,
            events,
            Arc::new(Notify::new()),
            Arc::new(crate::render_signal::RenderSignal::new()),
        );
        let ws = workspaces.remove(0);
        let cwds = ws.tabs[0]
            .panes
            .iter()
            .map(|(id, pane)| (*id, terminals[&pane.attached_terminal_id].cwd.clone()))
            .collect();
        (ws, cwds)
    }

    fn cwd_order(stack: &PaneStack, cwds: &HashMap<PaneId, PathBuf>) -> Vec<String> {
        stack
            .panes
            .iter()
            .map(|id| cwds[id].display().to_string())
            .collect()
    }

    #[test]
    fn capture_saves_stacks_and_hidden_panes_and_omits_an_empty_field() {
        let mut ws = Workspace::test_new("save");
        let root = ws.tabs[0].root_pane;
        let right = ws.test_split(ratatui::layout::Direction::Horizontal);
        let added = ws.test_stack(right);
        let snapshot = crate::persist::capture(
            std::slice::from_ref(&ws),
            &HashMap::new(),
            &crate::terminal::TerminalRuntimeRegistry::new(),
            Some(0),
            0,
        );
        let tab = &snapshot.workspaces[0].tabs[0];
        assert_eq!(
            tab.stacks,
            vec![StackSnapshot {
                panes: vec![right.raw(), added.raw()],
                active: 1
            }]
        );
        assert!(
            tab.panes.contains_key(&right.raw()),
            "the hidden pane is saved"
        );
        let json = serde_json::to_value(tab).unwrap();
        assert!(json.get("stacks").is_some());

        let plain = Workspace::test_new("plain");
        let snapshot = crate::persist::capture(
            std::slice::from_ref(&plain),
            &HashMap::new(),
            &crate::terminal::TerminalRuntimeRegistry::new(),
            Some(0),
            0,
        );
        let json = serde_json::to_value(&snapshot.workspaces[0].tabs[0]).unwrap();
        assert!(
            json.get("stacks").is_none(),
            "stock-identical without stacks"
        );
        let _ = root;
    }

    #[tokio::test]
    async fn a_saved_stack_restores_with_members_order_visible_pane_and_numbers() {
        let snapshot = stacked_session(
            vec![StackSnapshot {
                panes: vec![30, 20, 40],
                active: 1,
            }],
            10,
            20,
        );
        let (ws, cwds) = restore(&snapshot);
        ws.assert_invariants_for_test();
        let tab = &ws.tabs[0];
        assert_eq!(tab.layout.pane_count(), 2);
        assert_eq!(tab.panes.len(), 4);
        let stack = &tab.stacks[0];
        assert_eq!(cwd_order(stack, &cwds), ["/usr", "/tmp", "/etc"]);
        assert_eq!(stack.active, 1, "the tree's member is the visible one");
        assert_eq!(cwds[&tab.layout.focused()], PathBuf::from("/tmp"));
        let mut numbers: Vec<_> = tab
            .panes
            .keys()
            .map(|id| ws.public_pane_number(*id).unwrap())
            .collect();
        numbers.sort_unstable();
        assert_eq!(numbers, vec![1, 2, 5, 7], "hidden panes keep their numbers");
    }

    #[tokio::test]
    async fn a_hidden_root_keeps_the_tab_identity_and_a_hidden_focus_falls_back_to_a_leaf() {
        let snapshot = stacked_session(
            vec![StackSnapshot {
                panes: vec![20, 30],
                active: 0,
            }],
            30,
            30,
        );
        let (ws, cwds) = restore(&snapshot);
        ws.assert_invariants_for_test();
        let tab = &ws.tabs[0];
        assert_eq!(
            cwds[&tab.root_pane],
            PathBuf::from("/usr"),
            "root is the hidden pane"
        );
        assert!(tab.layout.pane_ids().contains(&tab.layout.focused()));
    }

    #[tokio::test]
    async fn damaged_saved_stacks_restore_repaired_without_error() {
        let snapshot = stacked_session(
            vec![
                // An unknown member is dropped.
                StackSnapshot {
                    panes: vec![20, 99, 30],
                    active: 7,
                },
                // 30 is already claimed and 10 is alone: dropped.
                StackSnapshot {
                    panes: vec![30, 10],
                    active: 0,
                },
                // No member is in the tree: dropped, 40 is not restored.
                StackSnapshot {
                    panes: vec![88, 40],
                    active: 0,
                },
            ],
            10,
            10,
        );
        let (ws, cwds) = restore(&snapshot);
        ws.assert_invariants_for_test();
        let tab = &ws.tabs[0];
        assert_eq!(tab.stacks.len(), 1);
        assert_eq!(cwd_order(&tab.stacks[0], &cwds), ["/tmp", "/usr"]);
        assert_eq!(tab.panes.len(), 3, "the slotless member is not restored");
    }

    #[test]
    fn a_stack_whose_visible_member_failed_hands_its_leaf_to_a_survivor() {
        let a = PaneId::alloc();
        let b = PaneId::alloc();
        let c = PaneId::alloc();
        let plans = vec![StackPlan {
            members: vec![1, 2, 3],
            visible: 1,
        }];
        let id_map = HashMap::from([(1, a), (2, b), (3, c)]);
        let mut node = Node::Pane(a);
        let surviving = HashSet::from([b, c]);
        let stacks = finish_stacks(&plans, &id_map, &surviving, &mut node);
        assert!(matches!(node, Node::Pane(id) if id == b));
        assert_eq!(
            stacks,
            vec![PaneStack {
                panes: vec![b, c],
                active: 0
            }]
        );

        let mut node = Node::Pane(a);
        let surviving = HashSet::from([c]);
        assert!(finish_stacks(&plans, &id_map, &surviving, &mut node).is_empty());
        assert!(
            matches!(node, Node::Pane(id) if id == c),
            "a lone survivor is a plain pane"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn handoff_aliases_cover_hidden_members() {
        let snapshot = stacked_session(
            vec![StackSnapshot {
                panes: vec![20, 30, 40],
                active: 0,
            }],
            10,
            10,
        );
        let (ws, cwds) = restore(&snapshot);
        let aliases = crate::persist::handoff_pane_aliases(&snapshot, std::slice::from_ref(&ws));
        assert_eq!(cwds[&aliases[&30]], PathBuf::from("/usr"));
        assert_eq!(cwds[&aliases[&40]], PathBuf::from("/etc"));
    }
}
