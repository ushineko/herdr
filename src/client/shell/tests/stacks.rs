//! Local patch: pane stacks in the client shell (spec 002).

use super::*;
use crate::protocol::endpoint::{
    PaneStackControl, PaneStackMember, PaneStackStrip, PaneStackSurface,
};

const COLS: u16 = 100;
const ROWS: u16 = 30;

/// pane_1 visible over a hidden pane_2, plus an unstacked pane_3.
fn stacked_state(supported: bool) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut snap = snapshot();
    for id in ["pane_2", "pane_3"] {
        let mut pane = snap.panes[0].clone();
        pane.pane_id = id.into();
        pane.focused = false;
        snap.panes.push(pane);
    }
    state.set_snapshot(Box::new(snap));
    state.set_pane_surface(super::surface());
    let local = super::super::ClientEndpointId::Local;
    state.set_endpoint_pane_stacks_supported(&local, supported);
    state.set_endpoint_pane_stacks(&local, stacks("boot-1"));
    state.compose(COLS, ROWS).expect("frame");
    state
}

fn stacks(boot_id: &str) -> PaneStackSurface {
    PaneStackSurface {
        boot_id: boot_id.into(),
        stacks: vec![PaneStackStrip {
            strip: crate::protocol::SurfaceRect {
                x: 0,
                y: 0,
                width: 40,
                height: 1,
            },
            visible: "pane_1".into(),
            members: vec![
                PaneStackMember {
                    pane_id: "pane_2".into(),
                    x: 0,
                    width: 20,
                    label: "yazi".into(),
                },
                PaneStackMember {
                    pane_id: "pane_1".into(),
                    x: 20,
                    width: 20,
                    label: "lazygit".into(),
                },
            ],
        }],
    }
}

fn stack_controls(outcome: &ClientShellInput) -> Vec<PaneStackControl> {
    outcome
        .requests
        .iter()
        .filter_map(|message| match message {
            ClientMessage::EndpointControl { kind, data }
                if kind == crate::protocol::endpoint::PANE_STACK_CONTROL_KIND =>
            {
                serde_json::from_str(data).ok()
            }
            _ => None,
        })
        .collect()
}

fn key(state: &mut ClientShellState, code: KeyCode, modifiers: KeyModifiers) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        code, modifiers,
    ))])
}

fn prefixed(state: &mut ClientShellState, code: KeyCode) -> ClientShellInput {
    key(state, KeyCode::Char('b'), KeyModifiers::CONTROL);
    key(state, code, KeyModifiers::empty())
}

fn mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn strip_cell(state: &ClientShellState, pane_id: &str) -> (u16, u16) {
    let hit = state
        .hits
        .stack_strips
        .iter()
        .find(|hit| hit.pane_id == pane_id)
        .expect("a strip entry");
    (hit.rect.x + 1, hit.rect.y)
}

#[test]
fn stack_keys_send_new_and_cycle_for_the_focused_pane() {
    let mut state = stacked_state(true);
    assert_eq!(
        stack_controls(&prefixed(&mut state, KeyCode::Char('t'))),
        vec![PaneStackControl::New {
            pane_id: Some("pane_1".into())
        }]
    );
    assert_eq!(
        stack_controls(&prefixed(&mut state, KeyCode::Char('.'))),
        vec![PaneStackControl::Cycle {
            pane_id: Some("pane_1".into()),
            forward: true
        }]
    );
    assert_eq!(
        stack_controls(&prefixed(&mut state, KeyCode::Char(','))),
        vec![PaneStackControl::Cycle {
            pane_id: Some("pane_1".into()),
            forward: false
        }]
    );
}

#[test]
fn a_server_without_pane_stacks_gets_no_stack_controls() {
    let mut state = stacked_state(false);
    let outcome = prefixed(&mut state, KeyCode::Char('t'));
    assert!(stack_controls(&outcome).is_empty());
    assert!(state.hits.stack_strips.is_empty(), "no stacks are kept");
}

#[test]
fn clicks_and_the_wheel_over_a_strip_switch_the_stack() {
    let mut state = stacked_state(true);
    let (column, row) = strip_cell(&state, "pane_2");
    let click = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
    );
    assert_eq!(
        stack_controls(&click),
        vec![PaneStackControl::Show {
            pane_id: "pane_2".into()
        }]
    );
    let wheel = mouse(&mut state, MouseEventKind::ScrollDown, column, row);
    assert_eq!(
        stack_controls(&wheel),
        vec![PaneStackControl::Cycle {
            pane_id: Some("pane_1".into()),
            forward: true
        }],
        "the wheel cycles from the visible member"
    );
    let below = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row + 2,
    );
    assert!(
        stack_controls(&below).is_empty(),
        "the pane below is not the strip"
    );
}

#[test]
fn stacks_from_another_server_boot_are_dropped() {
    let mut state = stacked_state(true);
    let local = super::super::ClientEndpointId::Local;
    assert!(!state.set_endpoint_pane_stacks(&local, stacks("old-boot")));
    assert!(
        state.hidden_pane_ids().contains("pane_2"),
        "the current ones stay"
    );
}

#[test]
fn pane_cycling_skips_hidden_members() {
    let mut state = stacked_state(true);
    assert_eq!(
        state.hidden_pane_ids(),
        std::collections::HashSet::from(["pane_2".to_string()])
    );
    let next = state.endpoint_method_for_action(crate::input::KeybindAction::CyclePaneNext);
    let focused = match next {
        Some(crate::api::schema::Method::PaneFocus(target)) => vec![target.pane_id],
        other => panic!("expected a pane focus, got {other:?}"),
    };
    assert_eq!(focused, vec!["pane_3".to_string()], "pane_2 is hidden");
}
