//! Local patch: dock rendering and client dock controls (spec 001).
//!
//! The dock is drawn once per change and sent as a named endpoint control to
//! every client that opted in by sending a dock control. Clients that never
//! did, including stock herdr clients, get nothing.

use super::*;
use crate::protocol::endpoint::{
    DockControl, DockSurface, DockSurfacePane, DockSurfaceSplit, DOCK_SURFACE_KIND,
};
use ratatui::style::Style;

impl HeadlessServer {
    /// Handle one dock control from a client. Returns whether to redraw.
    pub(super) fn handle_client_dock_control(&mut self, client_id: u64, data: String) -> bool {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };
        if !client.is_active_shell_client() {
            return false;
        }
        if !client.dock_opt_in {
            client.dock_opt_in = true;
            client.last_dock_message = None;
        }
        let control: DockControl = match serde_json::from_str(&data) {
            Ok(control) => control,
            Err(err) => {
                debug!(client_id, err = %err, "ignoring malformed dock control");
                return false;
            }
        };
        let redraw = match control {
            DockControl::Input { pane_id, events } => {
                if let Some(runtime) = self.app.dock_runtime(&pane_id) {
                    if let Err(err) = apply_client_pane_input_events(runtime, &events) {
                        debug!(client_id, %pane_id, err = %err, "dock input failed");
                    }
                }
                false
            }
            other => self.app.apply_dock_control(other),
        };
        // A newly opted-in client needs its first surface even if nothing changed.
        self.stream_dock_surfaces();
        redraw
    }

    /// Whether any of these dirty pty sources is a dock pane.
    pub(super) fn dock_sources_dirty(&self, sources: &HashSet<crate::layout::PaneId>) -> bool {
        sources.iter().any(|id| self.app.state.dock.contains(*id))
    }

    /// Send the current dock to every opted-in client whose last copy differs.
    pub(super) fn stream_dock_surfaces(&mut self) {
        let targets = self
            .clients
            .iter()
            .filter(|(_, client)| client.dock_opt_in && client.is_active_shell_client())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return;
        }
        let surface = self.render_dock_surface();
        let data = match serde_json::to_string(&surface) {
            Ok(data) => data,
            Err(err) => {
                warn!(err = %err, "failed to encode dock surface");
                return;
            }
        };
        for client_id in targets {
            let Some(client) = self.clients.get_mut(&client_id) else {
                continue;
            };
            if client.last_dock_message.as_deref() == Some(data.as_str()) {
                continue;
            }
            client.last_dock_message = Some(data.clone());
            self.send_to_client(
                client_id,
                ServerMessage::EndpointControl {
                    kind: DOCK_SURFACE_KIND.into(),
                    data: data.clone(),
                },
            );
        }
    }

    /// Draw the dock: each visible pane's screen, a separator between panes,
    /// and a restart notice in a seeded pane that is waiting to come back.
    pub(super) fn render_dock_surface(&self) -> DockSurface {
        let dock = &self.app.state.dock;
        let boot_id = self.client_shell_boot_id.clone();
        let Some(layout) = dock.open() else {
            return DockSurface {
                boot_id,
                rows: 0,
                cols: dock.width,
                frame: None,
                panes: Vec::new(),
                splits: Vec::new(),
                zoomed: false,
            };
        };
        let area = dock.area();
        let palette = &self.app.state.palette;
        let separator = Style::default().fg(palette.overlay0);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        let mut cursor = None;
        let mut panes = Vec::new();
        let now = Instant::now();
        for geometry in dock.geometry() {
            let Some(pane) = layout.panes.get(&geometry.id) else {
                continue;
            };
            let (rect, inner) = (geometry.rect, geometry.inner);
            if inner.width < rect.width {
                let x = rect.right() - 1;
                for y in rect.y..rect.bottom() {
                    buffer[(x, y)].set_symbol("│").set_style(separator);
                }
            }
            if inner.height < rect.height {
                let y = rect.bottom() - 1;
                for x in rect.x..rect.right() {
                    let symbol = if buffer[(x, y)].symbol() == "│" {
                        "┼"
                    } else {
                        "─"
                    };
                    buffer[(x, y)].set_symbol(symbol).set_style(separator);
                }
            }
            let runtime = pane
                .terminal_id
                .as_ref()
                .and_then(|terminal| self.app.terminal_runtimes.get(terminal));
            let restarting_in_ms = pane
                .respawn_at
                .map(|at| at.saturating_duration_since(now).as_millis() as u64);
            if let Some(runtime) = runtime {
                let content = Rect::new(0, 0, inner.width, inner.height);
                let (screen, pane_cursor) =
                    crate::server::render_stream::render_terminal_virtual(runtime, content);
                for y in 0..inner.height {
                    for x in 0..inner.width {
                        buffer[(inner.x + x, inner.y + y)] = screen[(x, y)].clone();
                    }
                }
                if geometry.focused {
                    cursor = pane_cursor.map(|mut state| {
                        state.x += inner.x;
                        state.y += inner.y;
                        state
                    });
                }
            } else if let Some(ms) = restarting_in_ms {
                let notice = format!(
                    "[{} exited, restarting in {}s]",
                    pane.label().unwrap_or("dock"),
                    ms.div_ceil(1000)
                );
                buffer.set_stringn(
                    inner.x,
                    inner.y,
                    &notice,
                    usize::from(inner.width),
                    Style::default().fg(palette.overlay0),
                );
            }
            panes.push(DockSurfacePane {
                pane_id: crate::app::dock::dock_pane_public_id(geometry.id),
                rect: rect.into(),
                inner_rect: inner.into(),
                focused: geometry.focused,
                mouse_reporting: runtime.is_some_and(|runtime| runtime.mouse_reporting_enabled()),
                sgr_pixel_mouse: runtime.is_some_and(|runtime| runtime.sgr_pixel_mouse_enabled()),
                label: pane.label().map(str::to_owned),
                restarting_in_ms,
            });
        }
        let splits = if layout.zoomed {
            Vec::new()
        } else {
            layout
                .layout
                .splits(area)
                .into_iter()
                .map(|split| DockSurfaceSplit {
                    vertical_divider: split.direction == ratatui::layout::Direction::Horizontal,
                    pos: split.pos,
                    area: split.area.into(),
                    path: split.path,
                })
                .collect()
        };
        DockSurface {
            boot_id,
            rows: area.height,
            cols: area.width,
            frame: Some(FrameData::from_ratatui_buffer_with_hyperlinks(
                &buffer,
                cursor,
                &[],
            )),
            panes,
            splits,
            zoomed: layout.zoomed,
        }
    }
}
