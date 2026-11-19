//! Local patch: pane stacks (spec 002, `ushineko/specs/002-nested-tabs.md`).
//!
//! A stack is a layout slot holding two or more panes. Only its visible
//! member is a leaf of the tab's `TileLayout`; the hidden members stay in
//! `Tab::panes` with their runtimes, so they remain ordinary panes to
//! everything that does not draw. Switching the visible member replaces the
//! leaf's id in place, so the rest of the layout never moves.

use super::Tab;
use crate::layout::{PaneId, TileLayout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneStack {
    /// Members in strip order. Always two or more.
    pub panes: Vec<PaneId>,
    /// Index of the visible member, the one in the layout tree.
    pub active: usize,
}

impl PaneStack {
    pub fn visible(&self) -> Option<PaneId> {
        self.panes.get(self.active).copied()
    }
}

// Stack logic over a layout and its stacks, shared by workspace tabs and the
// dock (spec 002).

pub(crate) fn stack_of(stacks: &[PaneStack], pane_id: PaneId) -> Option<&PaneStack> {
    stacks.iter().find(|stack| stack.panes.contains(&pane_id))
}

/// The layout leaves in layout order, then hidden members in strip order.
pub(crate) fn all_ids(layout: &TileLayout, stacks: &[PaneStack]) -> Vec<PaneId> {
    let mut ids = layout.pane_ids();
    for stack in stacks {
        for (index, pane_id) in stack.panes.iter().enumerate() {
            if index != stack.active {
                ids.push(*pane_id);
            }
        }
    }
    ids
}

pub(crate) fn is_hidden(stacks: &[PaneStack], pane_id: PaneId) -> bool {
    stack_of(stacks, pane_id).is_some_and(|stack| stack.visible() != Some(pane_id))
}

/// The leaf that holds a pane's slot: the pane itself, or its stack's visible
/// member.
pub(crate) fn slot_leaf(
    layout: &TileLayout,
    stacks: &[PaneStack],
    pane_id: PaneId,
) -> Option<PaneId> {
    if let Some(stack) = stack_of(stacks, pane_id) {
        return stack.visible();
    }
    layout.pane_ids().contains(&pane_id).then_some(pane_id)
}

/// The member after (or before) the visible one, wrapping.
pub(crate) fn neighbour(stacks: &[PaneStack], pane_id: PaneId, forward: bool) -> Option<PaneId> {
    let stack = stack_of(stacks, pane_id)?;
    let len = stack.panes.len();
    let next = if forward {
        (stack.active + 1) % len
    } else {
        (stack.active + len - 1) % len
    };
    stack.panes.get(next).copied()
}

/// Put `pane_id`, which is not in the layout, into `target`'s slot as the last
/// member of its stack, shown and focused. A plain slot becomes a stack.
pub(crate) fn push_member(
    layout: &mut TileLayout,
    stacks: &mut Vec<PaneStack>,
    target: PaneId,
    pane_id: PaneId,
) -> bool {
    let Some(leaf) = slot_leaf(layout, stacks, target) else {
        return false;
    };
    if stack_of(stacks, pane_id).is_some() || !layout.replace_pane(leaf, pane_id) {
        return false;
    }
    match stacks.iter_mut().find(|stack| stack.panes.contains(&leaf)) {
        Some(stack) => {
            stack.panes.push(pane_id);
            stack.active = stack.panes.len() - 1;
        }
        None => stacks.push(PaneStack {
            panes: vec![leaf, pane_id],
            active: 1,
        }),
    }
    layout.focus_pane(pane_id);
    true
}

/// Make a member visible, keeping focus where it is (it follows the slot if
/// the slot was focused). True when the pane is now visible.
pub(crate) fn reveal(layout: &mut TileLayout, stacks: &mut [PaneStack], pane_id: PaneId) -> bool {
    let Some(index) = stacks
        .iter()
        .position(|stack| stack.panes.contains(&pane_id))
    else {
        return layout.pane_ids().contains(&pane_id);
    };
    let stack = &stacks[index];
    let Some(visible) = stack.visible() else {
        return false;
    };
    if visible == pane_id {
        return true;
    }
    let Some(position) = stack.panes.iter().position(|id| *id == pane_id) else {
        return false;
    };
    if !layout.replace_pane(visible, pane_id) {
        return false;
    }
    stacks[index].active = position;
    true
}

/// Take a pane out of its stack, keeping the slot: a visible member hands its
/// leaf to its neighbour (the next member, else the previous one), and a stack
/// left with one member becomes a plain pane. False when the pane is not
/// stacked, so the caller removes it from the layout instead.
pub(crate) fn unstack(
    layout: &mut TileLayout,
    stacks: &mut Vec<PaneStack>,
    pane_id: PaneId,
) -> bool {
    let Some(index) = stacks
        .iter()
        .position(|stack| stack.panes.contains(&pane_id))
    else {
        return false;
    };
    let stack = &mut stacks[index];
    let Some(position) = stack.panes.iter().position(|id| *id == pane_id) else {
        return false;
    };
    if position == stack.active {
        let heir = if position + 1 < stack.panes.len() {
            position + 1
        } else {
            position - 1
        };
        let heir_id = stack.panes[heir];
        layout.replace_pane(pane_id, heir_id);
        let stack = &mut stacks[index];
        stack.panes.remove(position);
        // The next member slides into `position`; the previous one keeps its
        // index.
        stack.active = heir.min(position);
    } else {
        stack.panes.remove(position);
        if position < stack.active {
            stack.active -= 1;
        }
    }
    if stacks[index].panes.len() < 2 {
        stacks.remove(index);
    }
    true
}

/// Geometry for every pane: `infos` (the leaves) plus a copy of each hidden
/// member's visible member entry, so hidden panes are sized alike.
pub(crate) fn with_hidden(
    stacks: &[PaneStack],
    mut infos: Vec<crate::layout::PaneInfo>,
) -> Vec<crate::layout::PaneInfo> {
    for stack in stacks {
        let Some(visible) = stack.visible() else {
            continue;
        };
        let Some(info) = infos.iter().find(|info| info.id == visible).cloned() else {
            continue;
        };
        for (index, pane_id) in stack.panes.iter().enumerate() {
            if index != stack.active {
                let mut hidden = info.clone();
                hidden.id = *pane_id;
                infos.push(hidden);
            }
        }
    }
    infos
}

impl Tab {
    pub(crate) fn stack_of(&self, pane_id: PaneId) -> Option<&PaneStack> {
        stack_of(&self.stacks, pane_id)
    }

    /// Every pane of the tab: the layout leaves in layout order, then hidden
    /// stack members in strip order. Use it where a pane must not be missed
    /// (process lifecycle, cleanup); geometry uses the layout.
    pub(crate) fn all_pane_ids(&self) -> Vec<PaneId> {
        all_ids(&self.layout, &self.stacks)
    }

    /// Pane geometry for every pane: `infos` (the layout leaves) plus, for each
    /// hidden member, a copy of its visible member's entry. Hidden panes are
    /// sized like the visible one, so switching never reflows.
    pub(crate) fn with_hidden_members(
        &self,
        infos: Vec<crate::layout::PaneInfo>,
    ) -> Vec<crate::layout::PaneInfo> {
        with_hidden(&self.stacks, infos)
    }

    /// Whether the pane is a hidden member of a stack in this tab.
    pub(crate) fn is_hidden_pane(&self, pane_id: PaneId) -> bool {
        is_hidden(&self.stacks, pane_id)
    }

    /// The layout leaf that holds this pane's slot: the pane itself, or the
    /// visible member of its stack.
    pub(crate) fn slot_leaf(&self, pane_id: PaneId) -> Option<PaneId> {
        slot_leaf(&self.layout, &self.stacks, pane_id)
    }

    /// Fold `pane_id`, a layout leaf of this tab, into `target`'s slot as the
    /// last member of its stack, then show and focus it. A plain slot becomes
    /// a two-member stack. Every other slot keeps its place and size.
    pub(crate) fn stack_existing_pane(&mut self, target: PaneId, pane_id: PaneId) -> bool {
        let Some(leaf) = self.slot_leaf(target) else {
            return false;
        };
        if leaf == pane_id
            || self.stack_of(pane_id).is_some()
            || !self.layout.pane_ids().contains(&pane_id)
            || !self.layout.close_pane(pane_id)
        {
            return false;
        }
        push_member(&mut self.layout, &mut self.stacks, leaf, pane_id)
    }

    /// Make a member the visible pane of its stack, keeping focus where it is
    /// (it follows the slot if the slot was focused). Returns whether the pane
    /// is now visible (true for an unstacked layout pane).
    pub(crate) fn reveal_pane(&mut self, pane_id: PaneId) -> bool {
        reveal(&mut self.layout, &mut self.stacks, pane_id)
    }

    /// Reveal a pane (see `reveal_pane`) and focus it.
    pub(crate) fn show_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.reveal_pane(pane_id) {
            return false;
        }
        self.layout.focus_pane(pane_id);
        true
    }

    /// The next (or previous) member of `pane_id`'s stack after the visible
    /// one, wrapping.
    pub(crate) fn stack_neighbour(&self, pane_id: PaneId, forward: bool) -> Option<PaneId> {
        neighbour(&self.stacks, pane_id, forward)
    }

    /// Show the next (or previous) member and focus it. Returns it.
    #[cfg(test)]
    pub(crate) fn cycle_stack(&mut self, pane_id: PaneId, forward: bool) -> Option<PaneId> {
        let target = self.stack_neighbour(pane_id, forward)?;
        self.show_pane(target).then_some(target)
    }

    /// Take a pane out of its stack, keeping the slot. Returns false when the
    /// pane is not stacked, so the caller removes it from the layout instead.
    pub(super) fn unstack_pane(&mut self, pane_id: PaneId) -> bool {
        unstack(&mut self.layout, &mut self.stacks, pane_id)
    }

    /// Test-only: the stack rules on top of the workspace invariants.
    #[cfg(test)]
    pub(super) fn assert_stack_invariants_for_test(&self, workspace: &str, tab_idx: usize) {
        let leaves: std::collections::HashSet<_> = self.layout.pane_ids().into_iter().collect();
        let mut stacked = std::collections::HashSet::new();
        for stack in &self.stacks {
            assert!(
                stack.panes.len() >= 2,
                "workspace {workspace} tab {tab_idx} has a stack with fewer than two members"
            );
            assert!(
                stack.active < stack.panes.len(),
                "workspace {workspace} tab {tab_idx} stack active index out of range"
            );
            for (index, pane_id) in stack.panes.iter().enumerate() {
                assert!(
                    stacked.insert(*pane_id),
                    "workspace {workspace} tab {tab_idx} pane {pane_id:?} is in two stacks or twice in one"
                );
                assert!(
                    self.panes.contains_key(pane_id),
                    "workspace {workspace} tab {tab_idx} stack member {pane_id:?} has no pane state"
                );
                assert_eq!(
                    leaves.contains(pane_id),
                    index == stack.active,
                    "workspace {workspace} tab {tab_idx} stack member {pane_id:?}: only the visible member is a layout leaf"
                );
            }
        }
        let expected: std::collections::HashSet<_> = leaves.union(&stacked).copied().collect();
        let pane_set: std::collections::HashSet<_> = self.panes.keys().copied().collect();
        assert_eq!(
            pane_set, expected,
            "workspace {workspace} tab {tab_idx} panes must equal layout leaves plus hidden stack members"
        );
    }
}

#[cfg(test)]
impl super::Workspace {
    /// Split the focused pane and fold the new pane into `target`'s slot, as
    /// `stack_pane_shell` does without a process.
    pub(crate) fn test_stack(&mut self, target: PaneId) -> PaneId {
        let new_id = self.test_split(ratatui::layout::Direction::Horizontal);
        let tab_idx = self
            .find_tab_index_for_pane(target)
            .expect("stack target is in the workspace");
        assert!(
            self.tabs[tab_idx].stack_existing_pane(target, new_id),
            "test stack is built"
        );
        new_id
    }

    /// The adversarial identity fixture plus stacks: a three-member stack in
    /// its multi-pane tab (moved last by the fixture), and a one-slot stack
    /// making up the whole of its first tab. The active tab is unchanged.
    pub(crate) fn test_adversarial_identity_state_with_stacks() -> Self {
        let mut ws = Self::test_adversarial_identity_state();
        let active = ws.active_tab;
        let last = ws.tabs.len() - 1;
        ws.switch_tab(last);
        let target = ws.tabs[last].layout.focused();
        ws.test_stack(target);
        ws.test_stack(target);
        ws.switch_tab(0);
        let root = ws.tabs[0].root_pane;
        ws.test_stack(root);
        ws.switch_tab(active);
        ws.assert_invariants_for_test();
        ws
    }
}

#[cfg(test)]
mod tests {
    use super::super::Workspace;
    use super::*;
    use ratatui::layout::{Direction, Rect};

    const AREA: Rect = Rect::new(0, 0, 120, 40);

    fn rects(tab: &Tab) -> Vec<Rect> {
        tab.layout
            .panes(AREA)
            .into_iter()
            .map(|info| info.rect)
            .collect()
    }

    /// Two side-by-side panes, the right one stacked with a second member.
    fn stacked_split() -> (Workspace, PaneId, PaneId, PaneId) {
        let mut ws = Workspace::test_new("stacks");
        let left = ws.tabs[0].root_pane;
        let right = ws.test_split(Direction::Horizontal);
        let hidden_after = ws.test_stack(right);
        (ws, left, right, hidden_after)
    }

    #[test]
    fn stacking_a_pane_leaves_every_slot_in_place() {
        let mut ws = Workspace::test_new("stacks");
        let left = ws.tabs[0].root_pane;
        let right = ws.test_split(Direction::Horizontal);
        let before = rects(&ws.tabs[0]);

        let added = ws.test_stack(right);
        let tab = &ws.tabs[0];
        assert_eq!(rects(tab), before, "no slot moved or resized");
        assert_eq!(tab.layout.pane_ids(), vec![left, added]);
        assert_eq!(
            tab.layout.focused(),
            added,
            "the new member is shown and focused"
        );
        assert!(tab.is_hidden_pane(right));
        assert_eq!(tab.all_pane_ids(), vec![left, added, right]);
        ws.assert_invariants_for_test();
    }

    #[test]
    fn switching_changes_only_the_slot_and_keeps_order() {
        let (mut ws, left, right, added) = stacked_split();
        let before = rects(&ws.tabs[0]);
        let tab = &mut ws.tabs[0];

        assert_eq!(
            tab.cycle_stack(added, true),
            Some(right),
            "wraps to the first"
        );
        assert_eq!(tab.layout.pane_ids(), vec![left, right]);
        assert_eq!(tab.layout.focused(), right);
        assert_eq!(rects(tab), before);
        assert_eq!(tab.cycle_stack(right, false), Some(added));
        assert!(tab.show_pane(right));
        assert_eq!(tab.stack_of(right).unwrap().panes, vec![right, added]);
        assert!(tab.show_pane(left), "an unstacked leaf is already visible");
        assert_eq!(tab.layout.focused(), left);
        ws.assert_invariants_for_test();
    }

    #[test]
    fn swap_and_zoom_act_on_the_whole_slot() {
        let (mut ws, left, right, added) = stacked_split();
        let tab = &mut ws.tabs[0];
        assert!(tab.layout.swap_panes(left, added));
        assert_eq!(tab.layout.pane_ids(), vec![added, left], "the slot moved");
        assert_eq!(
            tab.stack_of(right).unwrap().panes,
            vec![right, added],
            "and its stack with it"
        );
        assert!(tab.show_pane(right));
        assert_eq!(
            tab.layout.pane_ids(),
            vec![right, left],
            "switching stays in the moved slot"
        );
        tab.zoomed = true;
        assert_eq!(tab.layout.focused(), right, "zoom shows the visible member");
        ws.assert_invariants_for_test();
    }

    #[test]
    fn closing_the_visible_member_shows_its_neighbour_then_collapses() {
        let (mut ws, left, right, added) = stacked_split();
        let third = ws.test_stack(right);
        assert!(ws.tabs[0].show_pane(added), "show the middle member");

        assert!(!ws.close_pane(added));
        let tab = &ws.tabs[0];
        assert_eq!(
            tab.layout.pane_ids(),
            vec![left, third],
            "the next member shows"
        );
        assert_eq!(tab.stack_of(third).unwrap().panes, vec![right, third]);
        ws.assert_invariants_for_test();

        assert!(!ws.close_pane(third));
        let tab = &ws.tabs[0];
        assert_eq!(
            tab.layout.pane_ids(),
            vec![left, right],
            "the previous member shows"
        );
        assert!(tab.stacks.is_empty(), "one member left is a plain pane");
        ws.assert_invariants_for_test();
    }

    #[test]
    fn closing_the_last_visible_member_shows_the_previous_one() {
        let (mut ws, left, right, added) = stacked_split();
        let third = ws.test_stack(right);
        assert_eq!(
            ws.tabs[0].stack_of(right).unwrap().panes,
            vec![right, added, third]
        );

        assert!(
            !ws.close_pane(third),
            "the last member, and the visible one"
        );
        let tab = &ws.tabs[0];
        assert_eq!(tab.layout.pane_ids(), vec![left, added]);
        let stack = tab.stack_of(added).unwrap();
        assert_eq!(stack.panes, vec![right, added]);
        assert_eq!(stack.visible(), Some(added));
        ws.assert_invariants_for_test();
    }

    #[test]
    fn closing_a_hidden_member_leaves_the_visible_one() {
        let (mut ws, left, right, added) = stacked_split();
        assert!(!ws.close_pane(right));
        assert_eq!(ws.tabs[0].layout.pane_ids(), vec![left, added]);
        assert!(ws.tabs[0].stacks.is_empty());
        assert_eq!(ws.public_pane_number(right), None, "its number is released");
        ws.assert_invariants_for_test();
    }

    /// Audit theme A: a one-slot stack is not a tab's last pane.
    #[test]
    fn a_member_of_a_one_slot_stack_is_not_the_last_pane() {
        for path in ["close", "death", "move"] {
            for take_root in [true, false] {
                let mut ws = Workspace::test_new("one-slot");
                let root = ws.tabs[0].root_pane;
                let added = ws.test_stack(root);
                let (pane, survivor) = if take_root {
                    (root, added)
                } else {
                    (added, root)
                };
                match path {
                    "close" => assert!(!ws.close_pane(pane), "{path}: workspace stays"),
                    "death" => assert!(!ws.remove_pane(pane), "{path}: workspace stays"),
                    _ => {
                        let taken = ws.take_pane_for_move(pane).expect("pane is taken");
                        assert_eq!(taken.removed_tab_idx, None, "{path}: tab stays");
                        assert!(!taken.workspace_empty);
                        ws.unregister_moved_pane(pane);
                    }
                }
                assert_eq!(ws.tabs.len(), 1, "{path} {pane:?}: the tab stays");
                let tab = &ws.tabs[0];
                assert_eq!(
                    tab.layout.pane_ids(),
                    vec![survivor],
                    "{path}: survivor shows"
                );
                assert!(tab.panes.contains_key(&survivor));
                assert!(tab.stacks.is_empty());
                ws.assert_invariants_for_test();
            }
        }
    }

    #[test]
    fn the_root_pane_passes_to_the_heir_when_its_member_closes() {
        let mut ws = Workspace::test_new("root");
        let root = ws.tabs[0].root_pane;
        let added = ws.test_stack(root);
        assert!(!ws.close_pane(root), "root is hidden; close it");
        assert_eq!(ws.tabs[0].root_pane, added);
        ws.assert_invariants_for_test();
    }

    #[test]
    fn closing_a_tab_releases_hidden_members() {
        let mut ws = Workspace::test_adversarial_identity_state_with_stacks();
        let last = ws.tabs.len() - 1;
        let hidden: Vec<_> = ws.tabs[last]
            .all_pane_ids()
            .into_iter()
            .filter(|id| ws.tabs[last].is_hidden_pane(*id))
            .collect();
        assert_eq!(hidden.len(), 2);
        assert!(ws.close_tab(last));
        for pane in hidden {
            assert_eq!(ws.public_pane_number(pane), None);
        }
        ws.assert_invariants_for_test();
    }

    #[test]
    fn the_adversarial_fixture_with_stacks_holds_the_invariants() {
        let plain = Workspace::test_adversarial_identity_state();
        let ws = Workspace::test_adversarial_identity_state_with_stacks();
        assert_eq!(
            ws.active_tab, plain.active_tab,
            "the fixture's active tab is kept"
        );
        let last = ws.tabs.len() - 1;
        assert_eq!(ws.tabs[last].stacks.len(), 1);
        assert_eq!(ws.tabs[last].stacks[0].panes.len(), 3);
        assert_eq!(ws.tabs[0].layout.pane_count(), 1);
        assert_eq!(
            ws.tabs[0].panes.len(),
            2,
            "a one-slot stack is the whole tab"
        );
    }

    fn app_with(workspace: Workspace) -> crate::app::AppState {
        let mut state = crate::app::AppState::test_new();
        state.workspaces = vec![workspace];
        state.active = Some(0);
        state.selected = 0;
        state.ensure_test_terminals();
        state
    }

    #[test]
    fn app_invariants_hold_with_the_stacked_adversarial_fixture() {
        app_with(Workspace::test_adversarial_identity_state_with_stacks())
            .assert_invariants_for_test();
    }

    #[test]
    fn closing_a_member_of_a_one_slot_stack_needs_no_workspace_confirmation() {
        let mut ws = Workspace::test_new("one-slot");
        let root = ws.tabs[0].root_pane;
        let added = ws.test_stack(root);
        let state = app_with(ws);
        assert!(!state.close_pane_would_close_workspace(0, root));
        assert!(!state.close_pane_would_close_workspace(0, added));
    }

    #[test]
    fn lifecycle_pane_lists_include_hidden_members() {
        let state = app_with(Workspace::test_adversarial_identity_state_with_stacks());
        let last = state.workspaces[0].tabs.len() - 1;
        let tab = &state.workspaces[0].tabs[last];
        let hidden: Vec<_> = tab
            .all_pane_ids()
            .into_iter()
            .filter(|id| tab.is_hidden_pane(*id))
            .collect();
        assert_eq!(hidden.len(), 2);
        let for_tab = state.pane_ids_for_tab(0, last);
        let for_workspace = state.pane_ids_for_workspace(0);
        for pane in hidden {
            assert!(
                for_tab.contains(&pane),
                "plugin cleanup on tab close sees it"
            );
            assert!(for_workspace.contains(&pane), "worktree removal sees it");
        }
    }

    #[test]
    #[should_panic(expected = "only the visible member is a layout leaf")]
    fn the_invariant_check_catches_a_hidden_leaf() {
        let (mut ws, _left, right, _added) = stacked_split();
        ws.tabs[0].stacks[0].active = ws.tabs[0].stacks[0]
            .panes
            .iter()
            .position(|id| *id == right)
            .unwrap();
        ws.assert_invariants_for_test();
    }

    #[test]
    #[should_panic(expected = "layout leaves plus hidden stack members")]
    fn the_invariant_check_catches_an_orphan_pane_state() {
        let (mut ws, _left, right, _added) = stacked_split();
        ws.tabs[0].stacks.clear();
        let _ = right;
        ws.assert_invariants_for_test();
    }
}
