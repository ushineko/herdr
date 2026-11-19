//! Local patch: the client side of pane stacks (spec 002,
//! `ushineko/specs/002-nested-tabs.md`).
//!
//! The server draws the strips into the pane surface and describes them in
//! `endpoint.pane-stack.v1`. This module turns the stack keys, strip clicks
//! and the wheel over a strip into `PaneStackControl`s, and tells the rest of
//! the client which snapshot panes are hidden.

use std::collections::HashSet;

use super::*;
use crate::protocol::endpoint::{PaneStackControl, PaneStackSurface};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

/// A strip entry on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StackStripHit {
    pub(super) rect: Rect,
    pub(super) pane_id: String,
    /// The visible member of the entry's stack.
    pub(super) visible: String,
    /// The stack is in the dock, so it is driven by dock controls.
    pub(super) dock: bool,
}

impl ClientShellState {
    pub(super) fn active_endpoint_offers_pane_stacks(&self) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.endpoint_id == self.active_endpoint_id && endpoint.pane_stacks_supported
        })
    }

    fn active_pane_stacks(&self) -> Option<&PaneStackSurface> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .filter(|endpoint| endpoint.pane_stacks_supported)
            .and_then(|endpoint| endpoint.pane_stacks.as_deref())
    }

    /// Snapshot panes that are hidden members of a stack: listed for Go To and
    /// notifications, but not on screen.
    pub(super) fn hidden_pane_ids(&self) -> HashSet<String> {
        self.active_pane_stacks()
            .map(|surface| {
                surface
                    .stacks
                    .iter()
                    .flat_map(|stack| {
                        stack
                            .members
                            .iter()
                            .filter(|member| member.pane_id != stack.visible)
                            .map(|member| member.pane_id.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn push_pane_stack_control(
        &self,
        control: PaneStackControl,
        outcome: &mut ClientShellInput,
    ) {
        if !self.active_endpoint_offers_pane_stacks() {
            return;
        }
        if let Ok(message) = crate::protocol::endpoint::pane_stack_control_message(&control) {
            outcome.requests.push(message);
            outcome.repaint = true;
        }
    }

    /// Handle a stack key binding. Returns whether it was one.
    pub(super) fn stack_binding(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction as A;
        if self.dock_has_focus() {
            return self.dock_stack_binding(action, outcome);
        }
        let pane_id = self.focused_pane_id();
        let control = match action {
            A::StackNewPane => PaneStackControl::New { pane_id },
            A::StackNext | A::StackPrevious => PaneStackControl::Cycle {
                pane_id,
                forward: action == A::StackNext,
            },
            _ => return false,
        };
        self.push_pane_stack_control(control, outcome);
        true
    }

    /// Strip entries on screen, from the latest pane-stack surface placed at
    /// the pane surface's origin.
    pub(super) fn stack_strip_hits(&self, pane_surface: Rect) -> Vec<StackStripHit> {
        let Some(surface) = self.active_pane_stacks() else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        for stack in &surface.stacks {
            let y = pane_surface.y.saturating_add(stack.strip.y);
            for member in &stack.members {
                hits.push(StackStripHit {
                    rect: Rect::new(pane_surface.x.saturating_add(member.x), y, member.width, 1),
                    pane_id: member.pane_id.clone(),
                    visible: stack.visible.clone(),
                    dock: false,
                });
            }
        }
        hits
    }

    /// Strip entries of the dock's stacks, placed in the dock's content area.
    pub(super) fn dock_stack_strip_hits(&self, dock_content: Rect) -> Vec<StackStripHit> {
        let Some(surface) = self.active_dock_surface() else {
            return Vec::new();
        };
        surface
            .stacks
            .iter()
            .flat_map(|stack| {
                stack.members.iter().map(move |member| StackStripHit {
                    rect: Rect::new(
                        dock_content.x.saturating_add(member.x),
                        dock_content.y.saturating_add(stack.strip.y),
                        member.width,
                        1,
                    ),
                    pane_id: member.pane_id.clone(),
                    visible: stack.visible.clone(),
                    dock: true,
                })
            })
            .collect()
    }

    /// Stack keys while the dock has the keyboard act on the dock's stack.
    fn dock_stack_binding(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction as A;
        use crate::protocol::endpoint::DockControl;
        let Some(pane_id) = self.focused_dock_pane_id() else {
            return true;
        };
        let control = match action {
            A::StackNewPane => DockControl::StackNew { pane_id },
            A::StackNext | A::StackPrevious => DockControl::StackCycle {
                pane_id,
                forward: action == A::StackNext,
            },
            _ => return false,
        };
        self.push_dock_control(control, outcome);
        outcome.repaint = true;
        true
    }

    /// Mouse events over a strip: a left click shows that member, the wheel
    /// cycles the stack. Returns whether the event was consumed.
    pub(super) fn handle_stack_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(hit) = self
            .hits
            .stack_strips
            .iter()
            .find(|hit| {
                mouse.row == hit.rect.y
                    && mouse.column >= hit.rect.x
                    && mouse.column < hit.rect.right()
            })
            .cloned()
        else {
            return false;
        };
        if hit.dock {
            use crate::protocol::endpoint::DockControl;
            let control = match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => DockControl::Focus {
                    pane_id: hit.pane_id,
                },
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => DockControl::StackCycle {
                    pane_id: hit.visible,
                    forward: mouse.kind == MouseEventKind::ScrollDown,
                },
                _ => return true,
            };
            self.push_dock_control(control, outcome);
            outcome.repaint = true;
            return true;
        }
        let control = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => PaneStackControl::Show {
                pane_id: hit.pane_id,
            },
            MouseEventKind::ScrollDown => PaneStackControl::Cycle {
                pane_id: Some(hit.visible),
                forward: true,
            },
            MouseEventKind::ScrollUp => PaneStackControl::Cycle {
                pane_id: Some(hit.visible),
                forward: false,
            },
            // The rest of a click, and moves, stay with the strip.
            _ => return true,
        };
        self.push_pane_stack_control(control, outcome);
        true
    }
}
