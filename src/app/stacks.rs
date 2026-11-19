//! Local patch: pane-stack actions (spec 002, `ushineko/specs/002-nested-tabs.md`).

use crate::api::schema::{EventData, EventEnvelope, EventKind, Method, PaneMoveDestination};
use crate::layout::PaneId;

use super::App;

/// How `stack_new_pane_with` adds a member.
pub(crate) struct StackNewOptions {
    pub cwd: Option<std::path::PathBuf>,
    pub label: Option<String>,
    pub show: bool,
    pub focus: bool,
}

impl App {
    /// Start a shell as a new member of `target`'s slot, shown and focused.
    pub(crate) fn stack_new_pane(
        &mut self,
        ws_idx: usize,
        target: PaneId,
    ) -> Result<PaneId, String> {
        self.stack_new_pane_with(
            ws_idx,
            target,
            StackNewOptions {
                cwd: None,
                label: None,
                show: true,
                focus: true,
            },
        )
    }

    /// Start a shell as a new member of `target`'s slot. Without `show` the
    /// slot keeps showing what it showed; without `focus` focus stays put.
    pub(crate) fn stack_new_pane_with(
        &mut self,
        ws_idx: usize,
        target: PaneId,
        options: StackNewOptions,
    ) -> Result<PaneId, String> {
        let slot = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(ws.find_tab_index_for_pane(target)?))
            .and_then(|tab| tab.slot_leaf(target))
            .ok_or_else(|| "pane not found".to_string())?;
        let (rows, cols) = self
            .state
            .new_pane_size(crate::ui::NewPanePlacement::Existing { ws_idx, pane: slot });
        let cwd = Some(match options.cwd {
            Some(cwd) => cwd,
            None => {
                let follow_cwd = self.launch_cwd_for_pane_in_workspace(ws_idx, slot);
                self.resolve_new_terminal_cwd(follow_cwd)
            }
        });
        let previous_leaf_focus = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(ws.find_tab_index_for_pane(slot)?))
            .map(|tab| tab.layout.focused());
        let default_shell = self.state.default_shell.clone();
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let host_terminal_appearance = self.state.host_terminal_appearance;
        let previous_focus = self.state.current_pane_focus_target();
        let shell_config = crate::pane::PaneShellConfig::new(&default_shell, self.state.shell_mode);
        let ws = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .ok_or_else(|| "pane not found".to_string())?;
        let (tab_idx, new_pane) = match ws.stack_pane_shell(
            slot,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            Vec::new(),
        ) {
            Some(Ok(created)) => created,
            Some(Err(err)) => return Err(err.to_string()),
            None => return Err("pane not found".to_string()),
        };
        let pane_id = new_pane.pane_id;
        let tab = &mut self.state.workspaces[ws_idx].tabs[tab_idx];
        if options.focus {
            // Shown and focused by `stack_pane_shell`.
        } else {
            if !options.show {
                tab.reveal_pane(slot);
            }
            if let Some(previous) = previous_leaf_focus {
                let previous = if previous == slot && options.show {
                    pane_id
                } else {
                    previous
                };
                tab.layout.focus_pane(previous);
            }
        }
        if options.focus {
            self.state.switch_workspace_tab(ws_idx, tab_idx);
            self.state
                .record_pane_focus_change(previous_focus, ws_idx, pane_id);
            self.state.mode = crate::app::Mode::Terminal;
        }
        self.terminal_runtimes
            .insert(new_pane.terminal.id.clone(), new_pane.runtime);
        self.state.remove_alias_shadowed_by_new_pane(pane_id);
        let mut terminal = new_pane.terminal;
        if let Some(label) = options.label.map(|label| label.trim().to_string()) {
            if !label.is_empty() {
                terminal.set_manual_label(label);
            }
        }
        self.state.terminals.insert(terminal.id.clone(), terminal);
        tracing::info!(pane = pane_id.raw(), slot = slot.raw(), "stack: new pane");
        if let Some(pane) = self.pane_info(ws_idx, pane_id) {
            self.emit_event(EventEnvelope {
                event: EventKind::PaneCreated,
                data: EventData::PaneCreated { pane },
            });
        }
        self.stack_layout_changed(ws_idx, tab_idx);
        Ok(pane_id)
    }

    /// `pane.stack`: a new nested tab in a pane's slot.
    pub(super) fn handle_pane_stack(
        &mut self,
        id: String,
        params: crate::api::schema::PaneStackParams,
    ) -> String {
        use super::api::responses::{encode_error, encode_success};
        let target = match params.target_pane_id.as_deref() {
            Some(pane_id) => self.parse_pane_id(pane_id),
            None => self.state.active.and_then(|ws_idx| {
                let pane = self.state.workspaces.get(ws_idx)?.focused_pane_id()?;
                Some((ws_idx, pane))
            }),
        };
        let Some((ws_idx, target)) = target else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let options = StackNewOptions {
            cwd: params.cwd.map(std::path::PathBuf::from),
            label: params.label,
            show: params.show || params.focus,
            focus: params.focus,
        };
        match self.stack_new_pane_with(ws_idx, target, options) {
            Ok(pane_id) => match self.pane_info(ws_idx, pane_id) {
                Some(pane) => {
                    encode_success(id, crate::api::schema::ResponseResult::PaneInfo { pane })
                }
                None => encode_error(id, "pane_not_found", "new pane is unavailable"),
            },
            Err(err) => encode_error(id, "pane_stack_failed", err),
        }
    }

    /// Show a stack member and focus it, as a strip click or a jump does.
    pub(crate) fn show_stacked_pane(&mut self, ws_idx: usize, pane_id: PaneId) -> bool {
        self.state.focus_pane_in_workspace(ws_idx, pane_id);
        let Some(tab_idx) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.find_tab_index_for_pane(pane_id))
        else {
            return false;
        };
        self.stack_layout_changed(ws_idx, tab_idx);
        true
    }

    /// Show the next (or previous) member of `pane_id`'s stack and focus it.
    pub(crate) fn cycle_stack(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        forward: bool,
    ) -> Option<PaneId> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let target = ws
            .tabs
            .get(ws.find_tab_index_for_pane(pane_id)?)?
            .stack_neighbour(pane_id, forward)?;
        self.show_stacked_pane(ws_idx, target).then_some(target)
    }

    /// Reveal hidden stack members named by a geometry request (split, swap,
    /// move, resize, zoom, directional focus), so the request acts on their
    /// slot. Decided for spec 002: reveal, then act.
    pub(super) fn reveal_geometry_targets(&mut self, method: &Method) {
        let ids: Vec<String> = match method {
            Method::PaneSplit(params) => params.target_pane_id.iter().cloned().collect(),
            Method::PaneSwap(params) => [
                &params.pane_id,
                &params.source_pane_id,
                &params.target_pane_id,
            ]
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
            Method::PaneMove(params) => {
                let mut ids = vec![params.pane_id.clone()];
                if let PaneMoveDestination::Tab {
                    target_pane_id: Some(target),
                    ..
                } = &params.destination
                {
                    ids.push(target.clone());
                }
                ids
            }
            Method::PaneResize(params) => params.pane_id.iter().cloned().collect(),
            Method::PaneZoom(params) => params.pane_id.iter().cloned().collect(),
            Method::PaneFocusDirection(params) => params.pane_id.iter().cloned().collect(),
            _ => return,
        };
        for id in ids {
            if let Some((ws_idx, pane_id)) = self.parse_pane_id(&id) {
                self.reveal_stacked_pane(ws_idx, pane_id);
            }
        }
    }

    /// Make a hidden member visible without moving focus. Returns whether
    /// anything changed.
    pub(crate) fn reveal_stacked_pane(&mut self, ws_idx: usize, pane_id: PaneId) -> bool {
        let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
            return false;
        };
        let Some(tab_idx) = ws.find_tab_index_for_pane(pane_id) else {
            return false;
        };
        let tab = &mut ws.tabs[tab_idx];
        if !tab.is_hidden_pane(pane_id) || !tab.reveal_pane(pane_id) {
            return false;
        }
        self.stack_layout_changed(ws_idx, tab_idx);
        true
    }

    /// A stack's visible member changed: save, redraw (a full render, so the
    /// retained-surface path recomputes the layout) and tell API listeners.
    pub(crate) fn stack_layout_changed(&mut self, ws_idx: usize, tab_idx: usize) {
        self.state.mark_session_dirty();
        self.schedule_session_save();
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
        self.emit_layout_updated_event(ws_idx, tab_idx);
    }
}
