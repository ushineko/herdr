//! Local patch: pane stacks over a headless server (spec 002).

use super::*;
use crate::protocol::endpoint::{PaneStackControl, PaneStackSurface, PANE_STACK_SURFACE_KIND};

struct Stacked {
    server: HeadlessServer,
    left: crate::layout::PaneId,
    hidden: crate::layout::PaneId,
    shown: crate::layout::PaneId,
}

/// `[left | shown]`, where `shown` stacks over `hidden`. Each pane's screen
/// shows its name; the stack members are labelled `yazi` and `lazygit`.
fn stacked_server() -> Stacked {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("stacks");
    let left = workspace.tabs[0].root_pane;
    let hidden = workspace.test_split(ratatui::layout::Direction::Horizontal);
    let shown = workspace.test_stack(hidden);
    for (pane, screen) in [
        (left, &b"LEFT-PANE"[..]),
        (hidden, b"HIDDEN-PANE"),
        (shown, b"SHOWN-PANE"),
    ] {
        workspace.insert_test_runtime(
            pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(40, 20, screen),
        );
    }
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    server.app.state.ensure_test_terminals();
    for (pane, label) in [(hidden, "yazi"), (shown, "lazygit")] {
        let terminal = server.app.state.workspaces[0]
            .terminal_id(pane)
            .cloned()
            .unwrap();
        server
            .app
            .state
            .terminals
            .get_mut(&terminal)
            .unwrap()
            .set_manual_label(label.into());
    }
    Stacked {
        server,
        left,
        hidden,
        shown,
    }
}

fn surfaces(
    render_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
) -> Vec<crate::protocol::PaneSurfaceFrame> {
    let mut out = Vec::new();
    while let Ok(bytes) = render_rx.recv_timeout(Duration::from_millis(200)) {
        if let ServerMessage::PaneSurface(surface) = read_server_message(bytes) {
            out.push(surface);
        }
    }
    out
}

fn stack_messages(control_rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<PaneStackSurface> {
    let mut out = Vec::new();
    while let Ok(bytes) = control_rx.recv_timeout(Duration::from_millis(200)) {
        if let ServerMessage::EndpointControl { kind, data } = read_server_message(bytes) {
            if kind == PANE_STACK_SURFACE_KIND {
                out.push(serde_json::from_str(&data).expect("decode pane stacks"));
            }
        }
    }
    out
}

fn control(server: &mut HeadlessServer, control: PaneStackControl) -> bool {
    server.handle_server_event(ServerEvent::ClientPaneStackControl {
        client_id: 7,
        data: serde_json::to_string(&control).unwrap(),
    })
}

fn public(server: &HeadlessServer, pane: crate::layout::PaneId) -> String {
    server.app.public_pane_id(0, pane).unwrap()
}

#[tokio::test]
async fn a_stack_draws_its_strip_and_only_its_visible_member() {
    let Stacked {
        mut server,
        left,
        hidden,
        shown,
    } = stacked_server();
    let (control_rx, render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    server.render_and_stream();

    let surface = surfaces(&render_rx).pop().expect("a pane surface");
    let text = frame_text(&surface.frame);
    assert!(text.contains("SHOWN-PANE") && text.contains("LEFT-PANE"));
    assert!(
        !text.contains("HIDDEN-PANE"),
        "the hidden member is not drawn"
    );
    assert!(
        text.contains("yazi") && text.contains("lazygit"),
        "strip labels: {text}"
    );
    assert_eq!(
        surface.panes.len(),
        2,
        "hidden members are not surface panes"
    );
    let shown_pane = surface
        .panes
        .iter()
        .find(|pane| pane.pane_id == public(&server, shown))
        .unwrap();
    let left_pane = surface
        .panes
        .iter()
        .find(|pane| pane.pane_id == public(&server, left))
        .unwrap();
    assert_eq!(
        shown_pane.inner_rect.height + 1,
        left_pane.inner_rect.height,
        "the strip takes one row from the stacked slot"
    );

    let stacks = stack_messages(&control_rx)
        .pop()
        .expect("a pane-stack message");
    assert_eq!(stacks.stacks.len(), 1);
    let strip = &stacks.stacks[0];
    assert_eq!(strip.visible, public(&server, shown));
    let members: Vec<_> = strip.members.iter().map(|m| m.pane_id.clone()).collect();
    assert_eq!(
        members,
        vec![public(&server, hidden), public(&server, shown)]
    );
    assert_eq!(strip.strip.y + 1, shown_pane.inner_rect.y);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn hidden_members_are_sized_like_the_visible_one() {
    let Stacked {
        mut server,
        hidden,
        shown,
        ..
    } = stacked_server();
    let (_control_rx, _render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    server.render_and_stream();
    let size = |pane| {
        server
            .app
            .state
            .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane)
            .map(|runtime| runtime.current_size())
            .unwrap()
    };
    assert_eq!(size(hidden), size(shown));
    assert_ne!(size(hidden), (20, 40), "resized from its test size");
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn hidden_member_output_takes_the_hidden_source_early_exit() {
    let Stacked {
        mut server,
        hidden,
        shown,
        ..
    } = stacked_server();
    let (_control_rx, _render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    server.render_and_stream();
    assert!(!server.pty_sources_visible_to_any_render_target(&HashSet::from([hidden])));
    assert!(server.pty_sources_visible_to_any_render_target(&HashSet::from([shown])));
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn show_and_cycle_controls_switch_the_slot_and_unknown_ones_are_ignored() {
    let Stacked {
        mut server,
        hidden,
        shown,
        ..
    } = stacked_server();
    let (_control_rx, _render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    let hidden_id = public(&server, hidden);
    assert!(control(
        &mut server,
        PaneStackControl::Show { pane_id: hidden_id }
    ));
    let tab = &server.app.state.workspaces[0].tabs[0];
    assert!(tab.is_hidden_pane(shown) && !tab.is_hidden_pane(hidden));
    assert_eq!(tab.layout.focused(), hidden, "the shown member takes focus");

    assert!(control(
        &mut server,
        PaneStackControl::Cycle {
            pane_id: None,
            forward: true
        }
    ));
    assert!(!server.app.state.workspaces[0].tabs[0].is_hidden_pane(shown));

    assert!(!control(&mut server, PaneStackControl::Unknown));
    assert!(
        !server.handle_server_event(ServerEvent::ClientPaneStackControl {
            client_id: 7,
            data: r#"{"op":"from_the_future"}"#.into(),
        })
    );
    assert!(
        !server.handle_server_event(ServerEvent::ClientPaneStackControl {
            client_id: 7,
            data: "not json".into(),
        })
    );
    server.app.state.workspaces[0].assert_invariants_for_test();
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn the_new_control_starts_a_shell_in_the_focused_slot() {
    let Stacked {
        mut server,
        left,
        shown,
        ..
    } = stacked_server();
    let (_control_rx, _render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    let leaves_before = server.app.state.workspaces[0].tabs[0].layout.pane_count();
    let left_id = public(&server, left);
    assert!(control(
        &mut server,
        PaneStackControl::New {
            pane_id: Some(left_id)
        }
    ));
    let tab = &server.app.state.workspaces[0].tabs[0];
    assert_eq!(tab.layout.pane_count(), leaves_before, "no new slot");
    assert_eq!(tab.stacks.len(), 2, "the left pane is now a stack too");
    let new_pane = tab.layout.focused();
    assert!(tab.stack_of(left).unwrap().panes.ends_with(&[new_pane]));
    assert!(tab.is_hidden_pane(left));
    assert!(!tab.is_hidden_pane(shown));
    server.app.state.workspaces[0].assert_invariants_for_test();
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn stack_free_views_send_no_pane_stack_messages() {
    let mut server = test_headless_server();
    install_shared_view_test_runtime(&mut server);
    let (control_rx, _render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    server.render_and_stream();
    assert!(stack_messages(&control_rx).is_empty());
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn a_stack_that_goes_away_is_reported_empty_once() {
    let Stacked {
        mut server, hidden, ..
    } = stacked_server();
    let (control_rx, _render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    server.render_and_stream();
    assert_eq!(stack_messages(&control_rx).len(), 1);
    assert!(!server.app.state.workspaces[0].close_pane(hidden));
    server.app.render_dirty.request_generic();
    server.render_and_stream();
    let messages = stack_messages(&control_rx);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].stacks.is_empty());
    server.render_and_stream();
    assert!(stack_messages(&control_rx).is_empty(), "and not again");
    shutdown_test_runtimes(&mut server);
}

fn api_call(
    server: &mut HeadlessServer,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let request: api::schema::Request = serde_json::from_value(serde_json::json!({
        "id": "stacks",
        "method": method,
        "params": params,
    }))
    .expect("a valid request");
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request,
        respond_to,
        response_write_complete: None,
    });
    serde_json::from_str(&response_rx.recv().unwrap()).unwrap()
}

#[tokio::test]
async fn pane_list_reports_hidden_members_with_their_stack() {
    let Stacked {
        mut server,
        left,
        hidden,
        shown,
    } = stacked_server();
    let panes = headless_pane_list(&mut server);
    assert_eq!(panes.len(), 3, "hidden members are listed");
    let find = |pane| {
        let id = public(&server, pane);
        panes
            .iter()
            .find(|info| info.pane_id == id)
            .unwrap()
            .clone()
    };
    assert_eq!(
        find(left).stack,
        None,
        "unstacked panes have no stack object"
    );
    let hidden_stack = find(hidden).stack.expect("hidden member has a stack");
    let shown_stack = find(shown).stack.expect("visible member has a stack");
    assert_eq!(hidden_stack.stack_id, public(&server, hidden));
    assert_eq!(shown_stack.stack_id, hidden_stack.stack_id);
    assert_eq!((hidden_stack.index, hidden_stack.visible), (0, false));
    assert_eq!((shown_stack.index, shown_stack.visible), (1, true));
    let json = serde_json::to_value(find(left)).unwrap();
    assert!(
        json.get("stack").is_none(),
        "omitted, so existing consumers see no change"
    );

    let workspace_id = server.app.public_workspace_id(0);
    let response = api_call(
        &mut server,
        "pane.list",
        serde_json::json!({"workspace_id": workspace_id}),
    );
    assert_eq!(
        response["result"]["panes"].as_array().map(Vec::len),
        Some(3),
        "a workspace-filtered list includes hidden members too: {response}"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn a_hidden_member_can_be_read_without_being_shown() {
    let Stacked {
        mut server, hidden, ..
    } = stacked_server();
    let pane_id = public(&server, hidden);
    let response = api_call(
        &mut server,
        "pane.read",
        serde_json::json!({"pane_id": pane_id, "source": "visible"}),
    );
    let text = response["result"]["read"]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("HIDDEN-PANE"), "{response}");
    assert!(
        server.app.state.workspaces[0].tabs[0].is_hidden_pane(hidden),
        "reading does not reveal"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn focusing_a_hidden_member_shows_it() {
    let Stacked {
        mut server, hidden, ..
    } = stacked_server();
    let pane_id = public(&server, hidden);
    let response = api_call(
        &mut server,
        "pane.focus",
        serde_json::json!({"pane_id": pane_id}),
    );
    assert!(response.get("error").is_none(), "{response}");
    let tab = &server.app.state.workspaces[0].tabs[0];
    assert!(!tab.is_hidden_pane(hidden));
    assert_eq!(tab.layout.focused(), hidden);
    assert!(tab.panes[&hidden].seen, "shown means seen");
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn geometry_requests_on_a_hidden_member_reveal_it_then_act() {
    let Stacked {
        mut server, hidden, ..
    } = stacked_server();
    let hidden_id = public(&server, hidden);
    let response = api_call(
        &mut server,
        "pane.split",
        serde_json::json!({"target_pane_id": hidden_id, "direction": "down", "focus": false}),
    );
    assert!(response.get("error").is_none(), "{response}");
    let tab = &server.app.state.workspaces[0].tabs[0];
    assert!(!tab.is_hidden_pane(hidden), "revealed");
    assert_eq!(tab.layout.pane_count(), 3, "then split");
    server.app.state.workspaces[0].assert_invariants_for_test();

    // Moving a pane next to a hidden member: before the fix, the insert failed
    // and the moved pane was recovered into a new tab.
    let Stacked {
        mut server,
        left,
        hidden,
        shown,
    } = stacked_server();
    server.app.state.workspaces[0].tabs[0]
        .layout
        .focus_pane(left);
    let extra = server.app.state.workspaces[0].test_split(ratatui::layout::Direction::Vertical);
    server.app.state.ensure_test_terminals();
    let (extra_id, hidden_id, tab_id) = (
        public(&server, extra),
        public(&server, hidden),
        server.app.public_tab_id(0, 0).unwrap(),
    );
    let response = api_call(
        &mut server,
        "pane.move",
        serde_json::json!({
            "pane_id": extra_id,
            "destination": {"type": "tab", "tab_id": tab_id, "target_pane_id": hidden_id, "split": "down"},
        }),
    );
    assert!(response.get("error").is_none(), "{response}");
    let ws = &server.app.state.workspaces[0];
    assert_eq!(ws.tabs.len(), 1, "no recovery tab");
    assert!(!ws.tabs[0].is_hidden_pane(hidden));
    assert!(ws.tabs[0].is_hidden_pane(shown));
    ws.assert_invariants_for_test();
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn hidden_members_count_as_off_screen_for_notifications_and_seen() {
    let Stacked {
        mut server,
        left,
        hidden,
        shown,
    } = stacked_server();
    assert!(server.app.state.pane_is_in_active_tab(0, shown));
    assert!(!server.app.state.pane_is_in_active_tab(0, hidden));
    let ws = &mut server.app.state.workspaces[0];
    for pane in [left, hidden, shown] {
        ws.tabs[0].panes.get_mut(&pane).unwrap().seen = false;
    }
    ws.switch_tab(0);
    assert!(ws.tabs[0].panes[&shown].seen);
    assert!(
        !ws.tabs[0].panes[&hidden].seen,
        "a hidden member is not seen"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn a_hidden_agent_is_listed() {
    let Stacked {
        mut server, hidden, ..
    } = stacked_server();
    let terminal = server.app.state.workspaces[0]
        .terminal_id(hidden)
        .cloned()
        .unwrap();
    let _ = server
        .app
        .state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .set_detected_state_with_screen_signals_at(
            crate::detect::parse_agent_label("claude"),
            crate::detect::AgentState::Blocked,
            false,
            false,
            false,
            false,
            std::time::Instant::now(),
        );
    let response = api_call(&mut server, "agent.list", serde_json::json!({}));
    let hidden_id = public(&server, hidden);
    let listed = response["result"]["agents"]
        .as_array()
        .is_some_and(|agents| agents.iter().any(|agent| agent["pane_id"] == hidden_id));
    assert!(listed, "{response}");
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn a_blocked_agent_in_a_hidden_member_shows_its_glyph_in_the_strip() {
    let Stacked {
        mut server, hidden, ..
    } = stacked_server();
    let terminal = server.app.state.workspaces[0]
        .terminal_id(hidden)
        .cloned()
        .unwrap();
    let _ = server
        .app
        .state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .set_detected_state_with_screen_signals_at(
            crate::detect::parse_agent_label("claude"),
            crate::detect::AgentState::Blocked,
            false,
            false,
            false,
            false,
            std::time::Instant::now(),
        );
    let (_control_rx, render_rx) = connect_test_shell(&mut server, 7, 80, 24);
    server.render_and_stream();
    let surface = surfaces(&render_rx).pop().expect("a pane surface");
    let text = frame_text(&surface.frame);
    assert!(
        text.contains("× yazi"),
        "the hidden blocked agent is flagged: {text}"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn moving_a_stacked_pane_to_a_new_workspace_takes_its_stack() {
    let Stacked {
        mut server,
        hidden,
        shown,
        ..
    } = stacked_server();
    let hidden_id = public(&server, hidden);
    let shown_id = public(&server, shown);
    let response = api_call(
        &mut server,
        "pane.move",
        serde_json::json!({"pane_id": hidden_id, "destination": {"type": "new_workspace"}}),
    );
    assert!(response.get("error").is_none(), "{response}");
    let workspaces = &server.app.state.workspaces;
    assert_eq!(workspaces.len(), 2);
    let moved = &workspaces[1].tabs[0];
    assert_eq!(
        moved.layout.pane_ids(),
        vec![hidden],
        "the named pane is shown"
    );
    assert!(moved.is_hidden_pane(shown), "its stack came along");
    assert_eq!(
        workspaces[0].tabs[0].panes.len(),
        1,
        "only the left pane stays"
    );
    for ws in workspaces {
        ws.assert_invariants_for_test();
    }
    assert_eq!(
        server.app.parse_pane_id(&hidden_id),
        Some((1, hidden)),
        "the old public id still resolves"
    );
    assert_eq!(
        server.app.parse_pane_id(&shown_id),
        Some((1, shown)),
        "so does a stack-mate's"
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn moving_a_stack_into_another_workspaces_tab_numbers_every_member() {
    let Stacked {
        mut server,
        hidden,
        shown,
        ..
    } = stacked_server();
    let mut other = crate::workspace::Workspace::test_new("other");
    let target = other.tabs[0].root_pane;
    other.insert_test_runtime(
        target,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(40, 20, b"OTHER"),
    );
    server.app.state.workspaces.push(other);
    server.app.state.ensure_test_terminals();
    let (shown_id, target_id, tab_id) = (
        public(&server, shown),
        server.app.public_pane_id(1, target).unwrap(),
        server.app.public_tab_id(1, 0).unwrap(),
    );
    let response = api_call(
        &mut server,
        "pane.move",
        serde_json::json!({
            "pane_id": shown_id,
            "destination": {"type": "tab", "tab_id": tab_id, "target_pane_id": target_id, "split": "right"},
        }),
    );
    assert!(response.get("error").is_none(), "{response}");
    let ws = &server.app.state.workspaces[1];
    assert!(ws.tabs[0].is_hidden_pane(hidden), "the stack arrived");
    assert!(
        ws.public_pane_number(hidden).is_some(),
        "its hidden member is numbered"
    );
    ws.assert_invariants_for_test();
    server.app.state.workspaces[0].assert_invariants_for_test();
    shutdown_test_runtimes(&mut server);
}
