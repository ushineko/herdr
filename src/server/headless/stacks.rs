//! Local patch: pane-stack controls and strip streaming (spec 002,
//! `ushineko/specs/002-nested-tabs.md`).

use super::*;
use crate::protocol::endpoint::{
    PaneStackControl, PaneStackStrip, PaneStackSurface, PANE_STACK_SURFACE_KIND,
};

impl HeadlessServer {
    /// Handle one pane-stack operation from a client. Returns whether to
    /// redraw.
    pub(super) fn handle_client_pane_stack_control(
        &mut self,
        client_id: u64,
        data: String,
    ) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        if !client.is_active_shell_client() {
            return false;
        }
        let control: PaneStackControl = match serde_json::from_str(&data) {
            Ok(control) => control,
            Err(err) => {
                debug!(client_id, err = %err, "ignoring malformed pane-stack control");
                return false;
            }
        };
        let resolve = |server: &Self, pane_id: Option<&str>| match pane_id {
            Some(pane_id) => server.app.parse_pane_id(pane_id),
            None => server.app.state.active.and_then(|ws_idx| {
                let pane = server.app.state.workspaces.get(ws_idx)?.focused_pane_id()?;
                Some((ws_idx, pane))
            }),
        };
        match control {
            PaneStackControl::New { pane_id } => {
                let Some((ws_idx, pane)) = resolve(self, pane_id.as_deref()) else {
                    return false;
                };
                match self.app.stack_new_pane(ws_idx, pane) {
                    Ok(_) => true,
                    Err(err) => {
                        warn!(client_id, err = %err, "stack: could not start a new pane");
                        false
                    }
                }
            }
            PaneStackControl::Show { pane_id } => resolve(self, Some(&pane_id))
                .is_some_and(|(ws_idx, pane)| self.app.show_stacked_pane(ws_idx, pane)),
            PaneStackControl::Cycle { pane_id, forward } => resolve(self, pane_id.as_deref())
                .and_then(|(ws_idx, pane)| self.app.cycle_stack(ws_idx, pane, forward))
                .is_some(),
            PaneStackControl::Unknown => false,
        }
    }

    /// Send a client the stacks in its view, when they changed. A view with no
    /// stacks sends nothing until it had some, so stack-free sessions and
    /// stock clients see no traffic.
    pub(super) fn send_pane_stacks(&mut self, client_id: u64, stacks: Vec<PaneStackStrip>) {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        if stacks.is_empty() && client.last_pane_stack_message.is_none() {
            return;
        }
        let surface = PaneStackSurface {
            boot_id: self.client_shell_boot_id.clone(),
            stacks,
        };
        let data = match serde_json::to_string(&surface) {
            Ok(data) => data,
            Err(err) => {
                warn!(err = %err, "failed to encode pane stacks");
                return;
            }
        };
        if client.last_pane_stack_message.as_deref() == Some(data.as_str()) {
            return;
        }
        client.last_pane_stack_message = (!surface.stacks.is_empty()).then(|| data.clone());
        self.send_to_client(
            client_id,
            ServerMessage::EndpointControl {
                kind: PANE_STACK_SURFACE_KIND.into(),
                data,
            },
        );
    }
}
