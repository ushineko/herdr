//! Local patch: the client side of the dock (spec 001, ushineko/specs/001-dock.md).
//!
//! The server draws the dock and sends it as a `DockSurface`; this module
//! places it along the bottom of the screen, routes keys and mouse events to
//! it while it holds focus, and turns pane keys, drags and the right-click
//! menu into `DockControl`s. Focus is client-side: the server's focused
//! workspace pane does not change while the keyboard is in the dock.

use super::*;
use crate::protocol::endpoint::{DockControl, DockDirection, DockSurfaceSplit};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

/// Clicks this close together on the dock's rule count as a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(350);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DockPaneHit {
    pub(super) rect: Rect,
    pub(super) inner: Rect,
    pub(super) pane_id: String,
    pub(super) mouse_reporting: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DockSplitHit {
    /// Screen cells that grab the divider.
    pub(super) hit: Rect,
    /// The split's area on screen, to turn a drag into a ratio.
    pub(super) area: Rect,
    pub(super) vertical_divider: bool,
    pub(super) path: Vec<bool>,
}

/// Hit areas of the dock as drawn this frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DockChromeHits {
    pub(super) rule: Rect,
    pub(super) panes: Vec<DockPaneHit>,
    pub(super) splits: Vec<DockSplitHit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DockDrag {
    /// Dragging the rule above the dock: the dock height.
    Rule,
    Split(DockSplitHit),
    /// A press forwarded to a dock program; its drags and release follow.
    PaneMouse(DockPaneHit),
}

fn dock_direction(action: crate::input::KeybindAction) -> Option<DockDirection> {
    use crate::input::KeybindAction as A;
    match action {
        A::FocusPaneLeft | A::ResizePaneLeft => Some(DockDirection::Left),
        A::FocusPaneRight | A::ResizePaneRight => Some(DockDirection::Right),
        A::FocusPaneUp | A::ResizePaneUp => Some(DockDirection::Up),
        A::FocusPaneDown | A::ResizePaneDown => Some(DockDirection::Down),
        _ => None,
    }
}

impl ClientShellState {
    /// Content rows the layout reserves for the dock on this client.
    pub(super) fn dock_layout_rows(&self) -> u16 {
        if self.dock_hidden {
            return 0;
        }
        self.active_dock_surface().map_or(0, |surface| surface.rows)
    }

    /// Whether the dock holds the keyboard right now. It loses focus as soon
    /// as it is no longer on screen.
    pub(super) fn dock_has_focus(&self) -> bool {
        self.dock_focused && !self.hits.dock_panes.is_empty()
    }

    /// Give the keyboard back once the active machine shows no dock: it was
    /// closed, reset, or the machine stopped offering one.
    pub(super) fn release_dock_focus_if_gone(&mut self) {
        if self.dock_layout_rows() == 0 {
            self.dock_focused = false;
        }
    }

    fn focused_dock_pane_id(&self) -> Option<String> {
        self.active_dock_surface()?
            .panes
            .iter()
            .find(|pane| pane.focused)
            .map(|pane| pane.pane_id.clone())
    }

    /// Where typed keys go: the focused dock pane while the dock has focus.
    pub(super) fn dock_keyboard_target(&self) -> Option<ClientInputTarget> {
        self.dock_has_focus()
            .then(|| self.focused_dock_pane_id().map(ClientInputTarget::Dock))
            .flatten()
    }

    fn active_endpoint_offers_dock(&self) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.endpoint_id == self.active_endpoint_id && endpoint.dock_supported
        })
    }

    /// Queue a dock control for the active endpoint. An operation from this
    /// client first claims the dock width, so the client acting on the dock
    /// sees it at its own width ("last to interact wins", as for shared tabs).
    pub(super) fn push_dock_control(
        &mut self,
        control: DockControl,
        outcome: &mut ClientShellInput,
    ) {
        if !self.active_endpoint_offers_dock() {
            return;
        }
        if !matches!(control, DockControl::Width { .. }) {
            self.claim_dock_width(outcome);
        }
        self.push_dock_message(control, outcome);
    }

    /// Size the dock to this client if another client last set its width.
    fn claim_dock_width(&mut self, outcome: &mut ClientShellInput) {
        let Some((cols, _)) = self.last_composed_size else {
            return;
        };
        let stale = self
            .active_dock_surface()
            .is_some_and(|surface| surface.cols != cols);
        if stale && self.active_endpoint_offers_dock() {
            self.push_dock_message(DockControl::Width { cols }, outcome);
        }
    }

    fn push_dock_message(&mut self, control: DockControl, outcome: &mut ClientShellInput) {
        if let Ok(message) = crate::protocol::endpoint::dock_control_message(&control) {
            outcome.requests.push(message);
        }
    }

    /// Report this client's width after its terminal resized.
    pub(crate) fn dock_width_message(&self, cols: u16) -> Option<ClientMessage> {
        self.active_endpoint_offers_dock()
            .then(|| {
                crate::protocol::endpoint::dock_control_message(&DockControl::Width { cols }).ok()
            })
            .flatten()
    }

    fn set_dock_focus(&mut self, focused: bool, outcome: &mut ClientShellInput) {
        if self.dock_focused != focused {
            self.dock_focused = focused;
            outcome.repaint = true;
        }
    }

    /// Handle a key binding that concerns the dock. Returns whether it was
    /// consumed: the dock's own bindings always are, and while the dock has
    /// focus so are the pane bindings that act on it.
    pub(super) fn dock_binding(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction as A;
        match action {
            A::FocusDock => {
                let visible = !self.hits.dock_panes.is_empty();
                let focus = visible && !self.dock_focused;
                if focus {
                    // Typing into the dock goes straight to its program, so
                    // claim the width when taking the keyboard.
                    self.claim_dock_width(outcome);
                }
                self.set_dock_focus(focus, outcome);
                return true;
            }
            A::ToggleDock => {
                self.dock_hidden = !self.dock_hidden;
                self.dock_hidden_manual = true;
                if self.dock_hidden {
                    self.dock_focused = false;
                }
                self.invalidate_pane_surface();
                outcome.repaint = true;
                outcome.resize = true;
                self.persist_chrome_preferences(outcome);
                return true;
            }
            A::ResetDock => {
                self.push_dock_control(DockControl::Reset, outcome);
                outcome.repaint = true;
                return true;
            }
            _ => {}
        }
        if !self.dock_has_focus() {
            return false;
        }
        let Some(focused) = self.focused_dock_pane_id() else {
            return false;
        };
        let control = match action {
            A::SplitVertical => DockControl::Split {
                pane_id: focused,
                right: true,
            },
            A::SplitHorizontal => DockControl::Split {
                pane_id: focused,
                right: false,
            },
            A::ClosePane => DockControl::Close { pane_id: focused },
            A::Zoom => DockControl::ToggleZoom,
            A::FocusPaneLeft | A::FocusPaneRight | A::FocusPaneUp | A::FocusPaneDown => {
                let Some(direction) = dock_direction(action) else {
                    return false;
                };
                DockControl::FocusDirection { direction }
            }
            A::ResizePaneLeft | A::ResizePaneRight | A::ResizePaneUp | A::ResizePaneDown => {
                let Some(direction) = dock_direction(action) else {
                    return false;
                };
                DockControl::Resize { direction }
            }
            _ => return false,
        };
        self.push_dock_control(control, outcome);
        outcome.repaint = true;
        true
    }

    /// Draw the rule above the dock and return the dock's hit areas.
    pub(super) fn render_dock_chrome(
        &self,
        buffer: &mut Buffer,
        layout: ClientShellLayout,
    ) -> DockChromeHits {
        let mut hits = DockChromeHits::default();
        if layout.dock.height < 2 {
            return hits;
        }
        let palette = &self.config.palette;
        let rule_style = Style::default().fg(if self.dock_focused {
            palette.accent
        } else {
            palette.overlay0
        });
        let rule = Rect::new(layout.dock.x, layout.dock.y, layout.dock.width, 1);
        for x in rule.x..rule.right() {
            buffer[(x, rule.y)].set_symbol("─").set_style(rule_style);
        }
        let content = Rect::new(
            layout.dock.x,
            layout.dock.y + 1,
            layout.dock.width,
            layout.dock.height - 1,
        );
        for y in content.y..content.bottom() {
            for x in content.x..content.right() {
                buffer[(x, y)].reset();
            }
        }
        if self.config.mouse_capture {
            hits.rule = rule;
        }
        let Some(surface) = self.active_dock_surface() else {
            return hits;
        };
        let on_screen = |x: u16, y: u16, width: u16, height: u16| {
            let rect = Rect::new(content.x + x, content.y + y, width, height);
            rect.intersection(content)
        };
        let panes = surface
            .panes
            .iter()
            .map(|pane| DockPaneHit {
                rect: on_screen(pane.rect.x, pane.rect.y, pane.rect.width, pane.rect.height),
                inner: on_screen(
                    pane.inner_rect.x,
                    pane.inner_rect.y,
                    pane.inner_rect.width,
                    pane.inner_rect.height,
                ),
                pane_id: pane.pane_id.clone(),
                mouse_reporting: pane.mouse_reporting,
            })
            .filter(|hit| !hit.inner.is_empty())
            .collect::<Vec<_>>();
        let splits = surface
            .splits
            .iter()
            .map(|split| split_hit(split, content))
            .collect::<Vec<_>>();
        hits.panes = panes;
        if self.config.mouse_capture {
            hits.splits = splits;
        }
        hits
    }

    /// Copy the dock frame into the composed frame. The cursor follows focus:
    /// the dock's while it has the keyboard, the workspace pane's otherwise.
    /// Pane graphics are left alone.
    pub(super) fn blit_dock(&self, frame: &mut FrameData, layout: ClientShellLayout) {
        if layout.dock.height < 2 {
            return;
        }
        let Some(dock_frame) = self
            .active_dock_surface()
            .and_then(|surface| surface.frame.as_ref())
        else {
            return;
        };
        let content = Rect::new(
            layout.dock.x,
            layout.dock.y + 1,
            layout.dock.width,
            layout.dock.height - 1,
        );
        let cursor = frame.cursor.clone();
        let graphics = std::mem::take(&mut frame.graphics);
        blit_pane_surface(frame, dock_frame, content);
        frame.graphics = graphics;
        if !self.dock_has_focus() {
            frame.cursor = cursor;
        }
    }

    /// Mouse events over the dock. Returns whether the event was consumed.
    pub(super) fn handle_dock_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let point = (mouse.column, mouse.row);
        if let Some(drag) = self.dock_drag.clone() {
            match mouse.kind {
                MouseEventKind::Drag(_) => {
                    self.drag_dock(&drag, mouse, outcome);
                    return true;
                }
                MouseEventKind::Up(_) => {
                    self.dock_drag = None;
                    if let DockDrag::PaneMouse(pane) = &drag {
                        self.forward_dock_mouse(pane, mouse, outcome);
                    }
                    return true;
                }
                _ => {}
            }
        }
        let over_rule = super::contains(self.hits.dock_rule, point);
        let pane = self
            .hits
            .dock_panes
            .iter()
            .find(|pane| super::contains(pane.rect, point))
            .cloned();
        let split = self
            .hits
            .dock_splits
            .iter()
            .find(|split| super::contains(split.hit, point))
            .cloned();
        if !over_rule && pane.is_none() && split.is_none() {
            // A press anywhere else hands the keyboard back to the workspace.
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.set_dock_focus(false, outcome);
            }
            return false;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if over_rule => {
                let now = std::time::Instant::now();
                let double = self
                    .last_dock_rule_click
                    .is_some_and(|last| now.duration_since(last) <= DOUBLE_CLICK);
                self.last_dock_rule_click = Some(now);
                if double {
                    self.push_dock_control(DockControl::ResetRows, outcome);
                } else {
                    self.dock_drag = Some(DockDrag::Rule);
                }
                outcome.repaint = true;
            }
            MouseEventKind::Down(MouseButton::Left) if split.is_some() => {
                self.dock_drag = split.map(DockDrag::Split);
            }
            MouseEventKind::Down(button) => {
                let Some(pane) = pane else {
                    return true;
                };
                self.set_dock_focus(true, outcome);
                self.push_dock_control(
                    DockControl::Focus {
                        pane_id: pane.pane_id.clone(),
                    },
                    outcome,
                );
                if button == MouseButton::Right && !pane.mouse_reporting {
                    self.open_dock_context_menu(pane.pane_id.clone(), mouse.column, mouse.row);
                    outcome.repaint = true;
                    return true;
                }
                if pane.mouse_reporting && super::contains(pane.inner, point) {
                    self.forward_dock_mouse(&pane, mouse, outcome);
                    self.dock_drag = Some(DockDrag::PaneMouse(pane));
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(pane) = pane.filter(|pane| super::contains(pane.inner, point)) {
                    self.forward_dock_mouse(&pane, mouse, outcome);
                }
            }
            MouseEventKind::Moved => {
                if let Some(pane) = pane.filter(|pane| pane.mouse_reporting) {
                    if super::contains(pane.inner, point) {
                        self.forward_dock_mouse(&pane, mouse, outcome);
                    }
                }
            }
            _ => {}
        }
        true
    }

    fn drag_dock(&mut self, drag: &DockDrag, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        match drag {
            DockDrag::Rule => {
                let Some((_, rows)) = self.last_composed_size else {
                    return;
                };
                // The rule sits on the dragged row; content fills the rest.
                let wanted = rows.saturating_sub(mouse.row).saturating_sub(1).max(1);
                let current = self.active_dock_surface().map_or(0, |surface| surface.rows);
                if wanted != current {
                    self.push_dock_control(DockControl::Rows { rows: wanted }, outcome);
                }
            }
            DockDrag::Split(split) => {
                let (offset, span) = if split.vertical_divider {
                    (mouse.column.saturating_sub(split.area.x), split.area.width)
                } else {
                    (mouse.row.saturating_sub(split.area.y), split.area.height)
                };
                if span > 1 {
                    let ratio = (f32::from(offset) + 1.0) / f32::from(span);
                    self.push_dock_control(
                        DockControl::Ratio {
                            path: split.path.clone(),
                            ratio: ratio.clamp(0.05, 0.95),
                        },
                        outcome,
                    );
                }
            }
            DockDrag::PaneMouse(pane) => self.forward_dock_mouse(pane, mouse, outcome),
        }
    }

    fn forward_dock_mouse(
        &self,
        pane: &DockPaneHit,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) {
        let Some(kind) = crate::protocol::ClientMouseKind::from_crossterm(mouse.kind) else {
            return;
        };
        let column = mouse
            .column
            .saturating_sub(pane.inner.x)
            .min(pane.inner.width.saturating_sub(1));
        let row = mouse
            .row
            .saturating_sub(pane.inner.y)
            .min(pane.inner.height.saturating_sub(1));
        super::push_target_event(
            ClientInputTarget::Dock(pane.pane_id.clone()),
            ClientPaneInputEvent::Mouse {
                kind,
                position: crate::protocol::ClientMousePosition::Cell { column, row },
                geometry: None,
                modifiers: mouse.modifiers.bits(),
                lines: self.config.mouse_scroll_lines.min(u16::MAX as usize) as u16,
            },
            outcome,
        );
    }

    fn open_dock_context_menu(&mut self, pane_id: String, x: u16, y: u16) {
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::DockPane { pane_id },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn activate_dock_context_action(
        &mut self,
        pane_id: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        let control = match action {
            ClientContextMenuAction::SplitRight => DockControl::Split {
                pane_id,
                right: true,
            },
            ClientContextMenuAction::SplitDown => DockControl::Split {
                pane_id,
                right: false,
            },
            ClientContextMenuAction::ClosePane => DockControl::Close { pane_id },
            ClientContextMenuAction::ResetDock => DockControl::Reset,
            _ => return,
        };
        self.push_dock_control(control, outcome);
    }
}

fn split_hit(split: &DockSurfaceSplit, content: Rect) -> DockSplitHit {
    let area = Rect::new(
        content.x + split.area.x,
        content.y + split.area.y,
        split.area.width,
        split.area.height,
    )
    .intersection(content);
    // The separator is the last cell of the first child; grab it and the cell
    // after it so the divider is easy to hit.
    let hit = if split.vertical_divider {
        Rect::new(
            (content.x + split.pos).saturating_sub(1),
            area.y,
            2,
            area.height,
        )
    } else {
        Rect::new(
            area.x,
            (content.y + split.pos).saturating_sub(1),
            area.width,
            2,
        )
    }
    .intersection(content);
    DockSplitHit {
        hit,
        area,
        vertical_divider: split.vertical_divider,
        path: split.path.clone(),
    }
}
