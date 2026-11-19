use super::*;
use crate::protocol::endpoint::DockSurface;

fn surface(boot_id: &str, rows: u16) -> DockSurface {
    DockSurface {
        boot_id: boot_id.into(),
        rows,
        cols: 80,
        frame: None,
        panes: Vec::new(),
        splits: Vec::new(),
        zoomed: false,
    }
}

fn shell_with_snapshot() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state
}

#[test]
fn a_dock_is_kept_only_from_an_endpoint_that_offers_it() {
    let mut state = shell_with_snapshot();
    let local = super::super::ClientEndpointId::Local;
    assert!(
        !state.set_endpoint_dock_surface(&local, surface("boot-1", 3)),
        "an endpoint without the capability cannot hand over a dock"
    );
    assert!(state.active_dock_surface().is_none());

    state.set_endpoint_dock_supported(&local, true);
    assert!(state.set_endpoint_dock_surface(&local, surface("boot-1", 3)));
    assert_eq!(state.active_dock_surface().map(|dock| dock.rows), Some(3));

    state.set_endpoint_dock_supported(&local, false);
    assert!(
        state.active_dock_surface().is_none(),
        "losing support drops the dock"
    );
}

#[test]
fn a_dock_from_another_server_boot_or_with_no_rows_is_not_shown() {
    let mut state = shell_with_snapshot();
    let local = super::super::ClientEndpointId::Local;
    state.set_endpoint_dock_supported(&local, true);
    assert!(
        !state.set_endpoint_dock_surface(&local, surface("stale-boot", 3)),
        "a surface from a previous server boot is dropped"
    );
    assert!(state.set_endpoint_dock_surface(&local, surface("boot-1", 0)));
    assert!(
        state.active_dock_surface().is_none(),
        "a closed dock draws nothing"
    );
}

// ---------------------------------------------------------------------------
// Layout, drawing, focus, keys and mouse (part 4)
// ---------------------------------------------------------------------------

use crate::input::TerminalKey;
use crate::protocol::endpoint::{DockControl, DockDirection, DockSurfacePane, DockSurfaceSplit};

const COLS: u16 = 100;
const ROWS: u16 = 30;

fn rect(x: u16, y: u16, width: u16, height: u16) -> crate::protocol::SurfaceRect {
    crate::protocol::SurfaceRect {
        x,
        y,
        width,
        height,
    }
}

/// Two side-by-side dock panes, "LEFT" focused and "RIGHT", 3 rows high.
fn two_pane_surface() -> DockSurface {
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, 3));
    buffer.set_string(0, 0, "LEFT", Style::default());
    buffer.set_string(50, 0, "RIGHT", Style::default());
    DockSurface {
        boot_id: "boot-1".into(),
        rows: 3,
        cols: COLS,
        frame: Some(FrameData::from_ratatui_buffer_with_hyperlinks(
            &buffer,
            None,
            &[],
        )),
        panes: vec![
            DockSurfacePane {
                pane_id: "dock_1".into(),
                rect: rect(0, 0, 50, 3),
                inner_rect: rect(0, 0, 49, 3),
                focused: true,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                label: Some("left".into()),
                restarting_in_ms: None,
            },
            DockSurfacePane {
                pane_id: "dock_2".into(),
                rect: rect(50, 0, 50, 3),
                inner_rect: rect(50, 0, 50, 3),
                focused: false,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                label: Some("right".into()),
                restarting_in_ms: None,
            },
        ],
        splits: vec![DockSurfaceSplit {
            vertical_divider: true,
            pos: 50,
            area: rect(0, 0, COLS, 3),
            path: Vec::new(),
        }],
        zoomed: false,
    }
}

fn docked_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(super::surface());
    let local = super::super::ClientEndpointId::Local;
    state.set_endpoint_dock_supported(&local, true);
    assert!(state.set_endpoint_dock_surface(&local, two_pane_surface()));
    state.compose(COLS, ROWS).expect("frame");
    state
}

fn dock_controls(outcome: &ClientShellInput) -> Vec<DockControl> {
    outcome
        .requests
        .iter()
        .filter_map(|message| match message {
            ClientMessage::EndpointControl { kind, data }
                if kind == crate::protocol::endpoint::DOCK_CONTROL_KIND =>
            {
                serde_json::from_str(data).ok()
            }
            _ => None,
        })
        .collect()
}

fn key(state: &mut ClientShellState, code: KeyCode, modifiers: KeyModifiers) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(code, modifiers))])
}

fn prefixed(
    state: &mut ClientShellState,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> ClientShellInput {
    key(state, KeyCode::Char('b'), KeyModifiers::CONTROL);
    key(state, code, modifiers)
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

#[test]
fn the_dock_takes_its_rows_plus_a_rule_at_the_bottom() {
    let state = docked_state();
    let layout = state.layout(COLS, ROWS);
    assert_eq!(layout.dock, Rect::new(0, ROWS - 4, COLS, 4));
    assert_eq!(
        layout.pane_surface.bottom(),
        ROWS - 4,
        "panes end above the dock"
    );
    assert_eq!(
        layout.sidebar.bottom(),
        ROWS - 4,
        "the sidebar ends above the dock"
    );
}

#[test]
fn without_a_dock_the_layout_is_unchanged() {
    let mut plain = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    plain.set_snapshot(Box::new(snapshot()));
    let docked = docked_state();
    let mut hidden = docked_state();
    hidden.dock_hidden = true;
    let base = plain.layout(COLS, ROWS);
    assert_eq!(base.dock, Rect::default());
    assert_eq!(
        hidden.layout(COLS, ROWS),
        base,
        "a hidden dock leaves the layout alone"
    );
    assert_ne!(docked.layout(COLS, ROWS), base);
}

#[test]
fn mobile_terminals_and_tiny_ones_keep_the_workspace() {
    let state = docked_state();
    assert_eq!(
        state.layout(60, ROWS).dock,
        Rect::default(),
        "no dock on mobile"
    );
    let small = state.layout(COLS, 6);
    assert_eq!(small.dock.height, 3, "the workspace keeps its minimum rows");
    assert_eq!(
        state.layout(COLS, 4).dock,
        Rect::default(),
        "too small to show a row"
    );
}

#[test]
fn a_bottom_tab_bar_sits_above_the_dock() {
    let mut config = Config::default();
    config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    let local = super::super::ClientEndpointId::Local;
    state.set_endpoint_dock_supported(&local, true);
    state.set_endpoint_dock_surface(&local, two_pane_surface());
    let layout = state.layout(COLS, ROWS);
    assert_eq!(layout.tab_bar.bottom(), layout.dock.y);
}

#[test]
fn compose_draws_the_rule_and_the_dock_frame() {
    let mut state = docked_state();
    let frame = state.compose(COLS, ROWS).expect("frame");
    let rows = frame_rows(&frame);
    assert!(
        rows[(ROWS - 4) as usize].chars().all(|ch| ch == '─'),
        "rule row"
    );
    let first = &rows[(ROWS - 3) as usize];
    assert!(first.starts_with("LEFT"), "{first:?}");
    assert!(first[50..].starts_with("RIGHT"), "{first:?}");
    assert_eq!(state.hits.dock_panes.len(), 2);
    assert_eq!(state.hits.dock_rule.y, ROWS - 4);
}

#[test]
fn focus_dock_routes_typed_keys_to_the_dock_and_back() {
    let mut state = docked_state();
    prefixed(&mut state, KeyCode::Char('u'), KeyModifiers::empty());
    assert!(state.dock_focused);
    let typed = key(&mut state, KeyCode::Char('q'), KeyModifiers::empty());
    assert!(matches!(
        &dock_controls(&typed)[..],
        [DockControl::Input { pane_id, .. }] if pane_id == "dock_1"
    ));
    assert!(
        !typed
            .requests
            .iter()
            .any(|message| matches!(message, ClientMessage::ClientShellPaneInput { .. })),
        "no workspace pane gets the key"
    );

    prefixed(&mut state, KeyCode::Char('u'), KeyModifiers::empty());
    assert!(!state.dock_focused);
    let typed = key(&mut state, KeyCode::Char('q'), KeyModifiers::empty());
    assert!(dock_controls(&typed).is_empty());
    assert!(typed
        .requests
        .iter()
        .any(|message| matches!(message, ClientMessage::ClientShellPaneInput { .. })));
}

#[test]
fn pane_keys_act_on_the_dock_only_while_it_has_focus() {
    let mut state = docked_state();
    let unfocused = prefixed(&mut state, KeyCode::Char('v'), KeyModifiers::empty());
    assert!(
        dock_controls(&unfocused).is_empty(),
        "a workspace split, not a dock one"
    );

    prefixed(&mut state, KeyCode::Char('u'), KeyModifiers::empty());
    let split = prefixed(&mut state, KeyCode::Char('v'), KeyModifiers::empty());
    assert_eq!(
        dock_controls(&split),
        vec![DockControl::Split {
            pane_id: "dock_1".into(),
            right: true
        }]
    );
    let zoom = prefixed(&mut state, KeyCode::Char('z'), KeyModifiers::empty());
    assert_eq!(dock_controls(&zoom), vec![DockControl::ToggleZoom]);
    let focus = prefixed(&mut state, KeyCode::Char('l'), KeyModifiers::empty());
    assert_eq!(
        dock_controls(&focus),
        vec![DockControl::FocusDirection {
            direction: DockDirection::Right
        }]
    );
    prefixed(&mut state, KeyCode::Char('r'), KeyModifiers::empty());
    let grow = key(&mut state, KeyCode::Char('k'), KeyModifiers::empty());
    assert_eq!(
        dock_controls(&grow),
        vec![DockControl::Resize {
            direction: DockDirection::Up
        }],
        "resize mode acts on the dock"
    );
}

#[test]
fn toggle_dock_hides_it_here_and_remembers() {
    let mut state = docked_state();
    prefixed(&mut state, KeyCode::Char('u'), KeyModifiers::empty());
    let outcome = prefixed(&mut state, KeyCode::Char('U'), KeyModifiers::SHIFT);
    assert!(state.dock_hidden && state.dock_hidden_manual);
    assert!(
        !state.dock_focused,
        "a hidden dock cannot hold the keyboard"
    );
    assert!(outcome.resize, "the pane area grows back");
    assert_eq!(state.layout(COLS, ROWS).dock, Rect::default());

    let mut config = ClientShellConfig::from_config(&Config::default());
    config.preferences.dock_hidden = Some(true);
    assert!(
        ClientShellState::new(config).dock_hidden,
        "restored from preferences"
    );
}

#[test]
fn clicks_focus_dock_panes_and_clicks_elsewhere_give_focus_back() {
    let mut state = docked_state();
    let row = ROWS - 2;
    let click = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), 60, row);
    assert!(state.dock_focused);
    assert!(dock_controls(&click).contains(&DockControl::Focus {
        pane_id: "dock_2".into()
    }));

    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), 60, row);
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), 30, 5);
    assert!(
        !state.dock_focused,
        "a click in the workspace returns the keyboard"
    );
}

#[test]
fn dragging_the_rule_sets_the_shared_height_and_double_click_resets_it() {
    let mut state = docked_state();
    let rule = ROWS - 4;
    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        10,
        rule,
    );
    let drag = mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        10,
        rule - 3,
    );
    assert_eq!(dock_controls(&drag), vec![DockControl::Rows { rows: 6 }]);
    mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        10,
        rule - 3,
    );

    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        10,
        rule,
    );
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), 10, rule);
    let double = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        10,
        rule,
    );
    assert_eq!(dock_controls(&double), vec![DockControl::ResetRows]);
}

#[test]
fn dragging_a_divider_sets_its_ratio() {
    let mut state = docked_state();
    let row = ROWS - 2;
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), 49, row);
    let drag = mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), 24, row);
    assert!(matches!(
        &dock_controls(&drag)[..],
        [DockControl::Ratio { path, ratio }] if path.is_empty() && (*ratio - 0.25).abs() < 0.01
    ));
}

#[test]
fn the_right_click_menu_offers_dock_actions() {
    let mut state = docked_state();
    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Right),
        10,
        ROWS - 2,
    );
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("right-click opens the dock menu");
    };
    let labels = menu
        .items()
        .iter()
        .map(|item| item.label.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "Split right",
            "Split down",
            "Close pane",
            "Reset dock to config"
        ]
    );
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(3, &mut outcome);
    assert_eq!(dock_controls(&outcome), vec![DockControl::Reset]);
}

#[test]
fn acting_on_the_dock_claims_its_width_first() {
    let mut state = docked_state();
    state.compose(120, ROWS).expect("a wider client");
    let focus = prefixed(&mut state, KeyCode::Char('u'), KeyModifiers::empty());
    assert_eq!(
        dock_controls(&focus),
        vec![DockControl::Width { cols: 120 }],
        "taking the keyboard claims the width"
    );
    let split = prefixed(&mut state, KeyCode::Char('v'), KeyModifiers::empty());
    assert_eq!(
        dock_controls(&split).first(),
        Some(&DockControl::Width { cols: 120 }),
        "so does any dock operation while the width is someone else's"
    );
}
