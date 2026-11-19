//! Local patch: dock surfaces and controls over a real headless server.

use super::*;
use crate::protocol::endpoint::{DockControl, DockSurface, DOCK_CONTROL_KIND, DOCK_SURFACE_KIND};

/// Connect a client shell. The second receiver must stay alive: the test
/// writer's delivery thread stops, and drops later messages, once either
/// receiver is gone.
fn connect_shell(
    server: &mut HeadlessServer,
    client_id: u64,
) -> (
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (writer, control_rx, render_rx) = test_client_writer();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            surface_reuse: false,
            surface_delta: false,
            surface_scroll: false,
            client_id,
            surface_cols: 100,
            surface_rows: 30,
            cell_width_px: 8,
            cell_height_px: 16,
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: true,
            surface_active: true,
            writer,
        })
    );
    (control_rx, render_rx)
}

/// Every dock surface delivered so far. The test writer delivers through a
/// background thread, so this waits for the stream to go quiet instead of
/// polling once.
fn dock_surfaces(receiver: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<DockSurface> {
    let mut surfaces = Vec::new();
    while let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(200)) {
        if let ServerMessage::EndpointControl { kind, data } = read_server_message(bytes) {
            if kind == DOCK_SURFACE_KIND {
                surfaces.push(serde_json::from_str(&data).expect("decode dock surface"));
            }
        }
    }
    surfaces
}

fn send(server: &mut HeadlessServer, client_id: u64, control: DockControl) -> bool {
    server.handle_server_event(ServerEvent::ClientDockControl {
        client_id,
        data: serde_json::to_string(&control).unwrap(),
    })
}

fn frame_text(surface: &DockSurface) -> Vec<String> {
    let frame = surface.frame.as_ref().expect("an open dock has a frame");
    frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol.as_str()).collect())
        .collect()
}

fn test_runtime(
    screen: &[u8],
) -> (
    crate::terminal::TerminalRuntime,
    tokio::sync::mpsc::Receiver<Bytes>,
) {
    crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, screen, 4)
}

#[tokio::test]
async fn clients_that_never_opt_in_get_no_dock_surfaces() {
    let mut server = test_headless_server();
    let (runtime, _input) = test_runtime(b"hello dock");
    server.app.install_test_dock(vec![("usage", runtime)]);
    let (control_rx, _render_rx) = connect_shell(&mut server, 7);

    server.render_and_stream();
    server.stream_dock_surfaces();
    assert!(
        dock_surfaces(&control_rx).is_empty(),
        "a client that sent no dock control, such as stock herdr, gets nothing"
    );
}

#[tokio::test]
async fn a_dock_control_opts_in_and_returns_the_drawn_dock() {
    let mut server = test_headless_server();
    let (runtime, _input) = test_runtime(b"hello dock");
    server.app.install_test_dock(vec![("usage", runtime)]);
    let (control_rx, _render_rx) = connect_shell(&mut server, 7);
    let _ = dock_surfaces(&control_rx);

    send(&mut server, 7, DockControl::Width { cols: 60 });
    let surfaces = dock_surfaces(&control_rx);
    assert_eq!(surfaces.len(), 1, "the opt-in returns the dock at once");
    let surface = &surfaces[0];
    assert_eq!(surface.boot_id, server.client_shell_boot_id);
    assert_eq!((surface.cols, surface.rows), (60, 3));
    assert_eq!(server.app.state.dock.width, 60);
    assert!(frame_text(surface)[0].starts_with("hello dock"));
    assert_eq!(surface.panes.len(), 1);
    assert!(surface.panes[0].pane_id.starts_with("dock_"));
    assert_eq!(surface.panes[0].label.as_deref(), Some("usage"));

    server.stream_dock_surfaces();
    assert!(
        dock_surfaces(&control_rx).is_empty(),
        "an unchanged dock is not resent"
    );
}

#[tokio::test]
async fn panes_render_side_by_side_with_a_separator() {
    let mut server = test_headless_server();
    let (left, _left_input) = test_runtime(b"LEFT");
    let (right, _right_input) = test_runtime(b"RIGHT");
    server
        .app
        .install_test_dock(vec![("a", left), ("b", right)]);
    let (control_rx, _render_rx) = connect_shell(&mut server, 7);
    send(&mut server, 7, DockControl::Width { cols: 40 });

    let surface = dock_surfaces(&control_rx).pop().expect("dock surface");
    let row = &frame_text(&surface)[0];
    assert!(row.starts_with("LEFT"), "{row:?}");
    assert!(
        row.contains("│RIGHT"),
        "a separator column precedes the right pane: {row:?}"
    );
    assert_eq!(surface.panes.len(), 2);
    assert_eq!(surface.splits.len(), 1);
    assert!(surface.splits[0].vertical_divider);
}

#[tokio::test]
async fn dock_input_reaches_the_dock_program_and_no_workspace_pane() {
    let mut server = test_headless_server();
    let mut workspace_input = install_focused_test_runtime(&mut server, b"");
    let (runtime, mut dock_input) = test_runtime(b"");
    let ids = server.app.install_test_dock(vec![("usage", runtime)]);
    let (_control_rx, _render_rx) = connect_shell(&mut server, 7);

    send(
        &mut server,
        7,
        DockControl::Input {
            pane_id: crate::app::dock::dock_pane_public_id(ids[0]),
            events: vec![crate::protocol::ClientPaneInputEvent::Paste("q".into())],
        },
    );
    let received = dock_input
        .try_recv()
        .expect("the dock program got the input");
    assert!(String::from_utf8_lossy(&received).contains('q'));
    assert!(
        workspace_input.try_recv().is_err(),
        "no workspace pane got it"
    );
}

#[tokio::test]
async fn a_waiting_seeded_pane_shows_a_restart_notice() {
    let mut server = test_headless_server();
    let (runtime, _input) = test_runtime(b"");
    let ids = server.app.install_test_dock(vec![("usage", runtime)]);
    server
        .app
        .handle_internal_event(crate::events::AppEvent::PaneDied {
            pane_id: ids[0],
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
    let (control_rx, _render_rx) = connect_shell(&mut server, 7);
    send(&mut server, 7, DockControl::Width { cols: 60 });

    let surface = dock_surfaces(&control_rx).pop().expect("dock surface");
    assert!(surface.panes[0].restarting_in_ms.is_some());
    assert!(
        frame_text(&surface)[0].starts_with("[usage exited, restarting in 1s]"),
        "{:?}",
        frame_text(&surface)[0]
    );
}

#[tokio::test]
async fn layout_controls_change_the_shared_dock() {
    let mut server = test_headless_server();
    let (left, _l) = test_runtime(b"");
    let (right, _r) = test_runtime(b"");
    let ids = server
        .app
        .install_test_dock(vec![("a", left), ("b", right)]);
    let (_control_rx, _render_rx) = connect_shell(&mut server, 7);
    send(&mut server, 7, DockControl::Width { cols: 80 });

    assert!(send(&mut server, 7, DockControl::Rows { rows: 6 }));
    assert_eq!(server.app.state.dock.rows, 6);
    assert!(send(&mut server, 7, DockControl::ResetRows));
    assert_eq!(server.app.state.dock.rows, 3);

    let left_id = crate::app::dock::dock_pane_public_id(ids[0]);
    assert!(send(
        &mut server,
        7,
        DockControl::Focus {
            pane_id: left_id.clone()
        }
    ));
    assert!(send(&mut server, 7, DockControl::ToggleZoom));
    assert!(server.app.state.dock.open().unwrap().zoomed);
    assert!(send(&mut server, 7, DockControl::ToggleZoom));

    // Up on a pane touching the dock's top edge grows the dock itself.
    assert!(send(
        &mut server,
        7,
        DockControl::Resize {
            direction: crate::protocol::endpoint::DockDirection::Up
        }
    ));
    assert_eq!(server.app.state.dock.rows, 4);

    assert!(send(
        &mut server,
        7,
        DockControl::Close {
            pane_id: left_id.clone()
        }
    ));
    assert_eq!(server.app.state.dock.open().unwrap().panes.len(), 1);
    assert!(
        !send(&mut server, 7, DockControl::Close { pane_id: left_id }),
        "a pane id that is gone is ignored"
    );
}

#[tokio::test]
async fn malformed_or_unknown_dock_controls_are_ignored() {
    let mut server = test_headless_server();
    let (runtime, _input) = test_runtime(b"");
    server.app.install_test_dock(vec![("usage", runtime)]);
    let (_control_rx, _render_rx) = connect_shell(&mut server, 7);
    assert!(!server.handle_server_event(ServerEvent::ClientDockControl {
        client_id: 7,
        data: "not json".into(),
    }));
    assert!(!server.handle_server_event(ServerEvent::ClientDockControl {
        client_id: 7,
        data: r#"{"op":"from_the_future"}"#.into(),
    }));
    let rows = server.app.state.dock.rows;
    for op in ["resize", "focus_direction"] {
        assert!(
            !server.handle_server_event(ServerEvent::ClientDockControl {
                client_id: 7,
                data: format!(r#"{{"op":"{op}","direction":"diagonal"}}"#),
            }),
            "an unknown {op} direction changes nothing"
        );
    }
    assert_eq!(server.app.state.dock.rows, rows);
    assert_eq!(server.app.state.dock.open().unwrap().panes.len(), 1);
    let _ = DOCK_CONTROL_KIND;
}

#[tokio::test]
async fn output_from_a_dock_pane_marks_the_dock_dirty() {
    let mut server = test_headless_server();
    install_focused_test_runtime(&mut server, b"");
    let (runtime, _input) = test_runtime(b"");
    let ids = server.app.install_test_dock(vec![("usage", runtime)]);
    let workspace_pane = server.app.state.workspaces[0].tabs[0].root_pane;

    assert!(server.dock_sources_dirty(&HashSet::from([ids[0]])));
    assert!(
        !server.dock_sources_dirty(&HashSet::from([workspace_pane])),
        "workspace output does not redraw the dock"
    );
}

#[tokio::test]
async fn full_renders_redraw_the_dock_only_after_it_changed() {
    let mut server = test_headless_server();
    install_focused_test_runtime(&mut server, b"");
    let (runtime, _input) = test_runtime(b"");
    let ids = server.app.install_test_dock(vec![("usage", runtime)]);
    let (control_rx, _render_rx) = connect_shell(&mut server, 7);
    send(&mut server, 7, DockControl::Width { cols: 60 });
    assert!(
        !dock_surfaces(&control_rx).is_empty(),
        "opting in draws the dock"
    );
    assert!(!server.app.state.dock.needs_redraw);

    server.render_and_stream();
    assert!(
        !server.app.state.dock.needs_redraw && dock_surfaces(&control_rx).is_empty(),
        "an unrelated full render leaves the dock alone"
    );

    server
        .app
        .handle_internal_event(crate::events::AppEvent::PaneDied {
            pane_id: ids[0],
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
    assert!(server.app.state.dock.needs_redraw, "an exit marks the dock");
    server.render_and_stream();
    let surface = dock_surfaces(&control_rx).pop().expect("the exit is drawn");
    assert!(surface.panes[0].restarting_in_ms.is_some());
    assert!(!server.app.state.dock.needs_redraw);
}

#[tokio::test]
async fn dock_stacks_are_drawn_and_driven_by_dock_controls() {
    let mut server = test_headless_server();
    let (runtime, _input) = test_runtime(b"");
    let ids = server.app.install_test_dock(vec![("usage", runtime)]);
    let (control_rx, _render_rx) = connect_shell(&mut server, 7);
    send(&mut server, 7, DockControl::Width { cols: 60 });
    let seeded = crate::app::dock::dock_pane_public_id(ids[0]);

    assert!(send(
        &mut server,
        7,
        DockControl::StackNew {
            pane_id: seeded.clone()
        }
    ));
    let surface = dock_surfaces(&control_rx).pop().expect("a dock surface");
    assert_eq!(surface.panes.len(), 1, "one slot");
    assert_eq!(surface.stacks.len(), 1);
    let members: Vec<_> = surface.stacks[0]
        .members
        .iter()
        .map(|member| member.pane_id.clone())
        .collect();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0], seeded);
    assert!(
        frame_text(&surface)[0].contains("usage"),
        "the strip is drawn"
    );
    assert!(server.app.state.dock.is_hidden(ids[0]));

    assert!(
        !server.dock_sources_dirty(&HashSet::from([ids[0]])),
        "a hidden dock member's output draws nothing"
    );
    assert!(
        !server.pty_sources_visible_to_any_render_target(&HashSet::from([ids[0]])),
        "dock output never needs a pane-surface render"
    );

    let shown = surface.stacks[0].visible.clone();
    assert!(send(
        &mut server,
        7,
        DockControl::StackCycle {
            pane_id: shown,
            forward: true
        }
    ));
    assert!(
        !server.app.state.dock.is_hidden(ids[0]),
        "cycled back to usage"
    );
}
