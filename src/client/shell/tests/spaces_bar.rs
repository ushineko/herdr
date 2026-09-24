//! Experimental horizontal spaces bar.

use super::*;

use crate::config::SidebarLayoutConfig;

/// A snapshot with `count` spaces, the first focused.
fn snapshot_with_spaces(count: usize) -> ClientShellSnapshot {
    let mut snapshot = snapshot();
    snapshot.workspaces = (0..count)
        .map(|index| ClientShellWorkspace {
            workspace_id: format!("ws_{}", index + 1),
            active_tab_id: "tab_1".into(),
            new_workspace_cwd: "/repo".into(),
            number: index + 1,
            label: format!("space-{}", index + 1),
            custom_label: false,
            branch: Some("main".into()),
            git_ahead_behind: None,
            tokens: Vec::new(),
            worktree: None,
            focused: index == 0,
            agent_status: AgentStatus::Idle,
        })
        .collect();
    snapshot
}

fn agent(pane_id: &str, workspace_id: &str, status: AgentStatus) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: workspace_id.into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: Some("claude".into()),
        agent: Some("claude".into()),
        title: Some(pane_id.into()),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

fn horizontal_state(snapshot: ClientShellSnapshot) -> ClientShellState {
    let mut config = Config::default();
    config.ui.sidebar_layout = SidebarLayoutConfig::Horizontal;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state
}

fn click_at(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn horizontal_bar_occupies_the_top_rows_and_leaves_the_terminal_below() {
    let mut state = horizontal_state(snapshot_with_spaces(3));
    state.compose(100, 30).expect("horizontal frame");

    let layout = state.layout(100, 30);
    assert_eq!(
        layout.spaces_bar,
        Rect::new(0, 0, 100, 2),
        "the bar spans the full width of the top two rows"
    );
    assert_eq!(
        layout.sidebar,
        Rect::default(),
        "the left rail gives up all of its columns"
    );
    assert_eq!(
        layout.pane_surface.x, 0,
        "the terminal starts at column 0 with no rail beside it"
    );
    assert!(
        layout.pane_surface.y >= 2,
        "the terminal starts below the bar, got y={}",
        layout.pane_surface.y
    );
}

#[test]
fn chips_are_laid_out_left_to_right_on_one_row() {
    let mut state = horizontal_state(snapshot_with_spaces(3));
    state.compose(100, 30).expect("horizontal frame");

    let chips = state
        .hits
        .spaces_bar
        .chips
        .iter()
        .filter(|chip| chip.rect.width > 0)
        .collect::<Vec<_>>();
    assert_eq!(chips.len(), 3, "every space gets a chip at this width");
    for chip in &chips {
        assert_eq!(chip.rect.height, 1, "chips are a single row tall");
        assert_eq!(chip.rect.y, 0, "chips sit on the bar's first row");
    }
    for pair in chips.windows(2) {
        assert!(
            pair[0].rect.x + pair[0].rect.width <= pair[1].rect.x,
            "chips run left to right without overlapping"
        );
    }
}

#[test]
fn hit_test_geometry_comes_from_the_rendered_layout() {
    let mut state = horizontal_state(snapshot_with_spaces(3));
    state.compose(100, 30).expect("horizontal frame");

    // Chips register through the same hit path the vertical rail uses, so the
    // rects that answer clicks are the rects that were drawn.
    for chip in state
        .hits
        .spaces_bar
        .chips
        .iter()
        .filter(|chip| chip.rect.width > 0)
    {
        let workspace_id = format!("ws_{}", chip.index + 1);
        let hit = state
            .hits
            .workspaces
            .iter()
            .find(|hit| hit.workspace_id == workspace_id)
            .expect("chip registers a workspace hit");
        assert_eq!(hit.rect, chip.rect);
    }
}

#[test]
fn clicking_a_chip_focuses_that_space() {
    let mut state = horizontal_state(snapshot_with_spaces(3));
    state.compose(100, 30).expect("horizontal frame");

    let target = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("second chip")
        .rect;
    click_at(&mut state, target.x + 1, target.y);
    let release = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: target.x + 1,
        row: target.y,
        modifiers: KeyModifiers::empty(),
    })]);

    let [ClientShellAction::Endpoint { request, .. }] = &release.actions[..] else {
        panic!("releasing on a chip should focus that space");
    };
    assert!(
        matches!(
            &request.method,
            crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_2"
        ),
        "the clicked chip's space is focused, got {:?}",
        request.method
    );
}

#[test]
fn a_lone_agent_is_focused_without_opening_a_picker() {
    let mut snapshot = snapshot_with_spaces(2);
    snapshot.agents = vec![agent("pane_a", "ws_1", AgentStatus::Blocked)];
    let mut state = horizontal_state(snapshot);
    state.compose(100, 30).expect("horizontal frame");

    let status_rect = state
        .hits
        .spaces_bar
        .chips
        .iter()
        .find(|chip| chip.index == 0)
        .expect("first chip")
        .status_rect;
    assert!(
        status_rect.width > 0,
        "a space with an agent shows a rollup"
    );

    let click = click_at(&mut state, status_rect.x, status_rect.y);
    let [ClientShellAction::Endpoint { request, .. }] = &click.actions[..] else {
        panic!(
            "one agent should focus straight through, got {:?}",
            click.actions.len()
        );
    };
    assert!(
        matches!(
            &request.method,
            crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_a"
        ),
        "the lone agent's pane is focused"
    );
    assert!(
        state.overlay.is_none(),
        "no picker opens for a single agent"
    );
}

#[test]
fn several_agents_open_a_picker_ordered_by_attention() {
    let mut snapshot = snapshot_with_spaces(2);
    snapshot.agents = vec![
        agent("pane_idle", "ws_1", AgentStatus::Idle),
        agent("pane_blocked", "ws_1", AgentStatus::Blocked),
        agent("pane_working", "ws_1", AgentStatus::Working),
    ];
    let mut state = horizontal_state(snapshot);
    state.compose(100, 30).expect("horizontal frame");

    let status_rect = state
        .hits
        .spaces_bar
        .chips
        .iter()
        .find(|chip| chip.index == 0)
        .expect("first chip")
        .status_rect;
    click_at(&mut state, status_rect.x, status_rect.y);

    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("several agents should open the picker");
    };
    let ClientContextMenuTarget::SpaceAgents { agents } = &menu.target else {
        panic!("the picker targets the chip's folded agents");
    };
    assert_eq!(
        agents
            .iter()
            .map(|entry| entry.pane_id.as_str())
            .collect::<Vec<_>>(),
        vec!["pane_blocked", "pane_working", "pane_idle"],
        "rows run most in need of attention first"
    );
}

#[test]
fn choosing_a_picker_row_focuses_that_agents_pane() {
    let mut snapshot = snapshot_with_spaces(2);
    snapshot.agents = vec![
        agent("pane_idle", "ws_1", AgentStatus::Idle),
        agent("pane_blocked", "ws_1", AgentStatus::Blocked),
    ];
    let mut state = horizontal_state(snapshot);
    state.compose(100, 30).expect("horizontal frame");

    let status_rect = state
        .hits
        .spaces_bar
        .chips
        .iter()
        .find(|chip| chip.index == 0)
        .expect("first chip")
        .status_rect;
    click_at(&mut state, status_rect.x, status_rect.y);

    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(1, &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("choosing a row focuses through the endpoint API");
    };
    assert!(
        matches!(
            &request.method,
            crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_idle"
        ),
        "the chosen row's pane is focused, not the first one"
    );
    assert!(state.overlay.is_none(), "the picker closes after choosing");
}

#[test]
fn toggling_layout_flips_placement_and_claims_it_from_config() {
    let config = Config::default();
    assert_eq!(config.ui.sidebar_layout, SidebarLayoutConfig::Vertical);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot_with_spaces(2)));
    state.set_pane_surface(surface());
    assert!(!state.sidebar_layout_manual, "config still owns the value");

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ToggleSidebarLayout),
        &mut outcome,
    );

    assert_eq!(state.sidebar_layout, SidebarLayoutConfig::Horizontal);
    assert!(
        state.sidebar_layout_manual,
        "a toggle claims the placement from config"
    );
    assert!(outcome.resize, "the pane surface has to be re-measured");

    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ToggleSidebarLayout),
        &mut ClientShellInput::default(),
    );
    assert_eq!(
        state.sidebar_layout,
        SidebarLayoutConfig::Vertical,
        "toggling again returns to the rail"
    );
}

#[test]
fn toggle_sidebar_is_inert_in_horizontal_placement() {
    let mut state = horizontal_state(snapshot_with_spaces(2));
    let collapsed = state.sidebar_collapsed;

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ToggleSidebar),
        &mut outcome,
    );

    assert_eq!(
        state.sidebar_collapsed, collapsed,
        "the bar has no collapsed form, so collapsing it would strand the user"
    );
}

#[test]
fn toggle_sidebar_still_collapses_the_vertical_rail() {
    let config = Config::default();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot_with_spaces(2)));
    state.set_pane_surface(surface());
    let collapsed = state.sidebar_collapsed;

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ToggleSidebar),
        &mut outcome,
    );

    assert_ne!(
        state.sidebar_collapsed, collapsed,
        "vertical placement keeps its collapse behaviour"
    );
}

#[test]
fn wheel_over_the_bar_scrolls_chips() {
    // Narrow enough that the chips overflow and the strip can actually scroll.
    let mut state = horizontal_state(snapshot_with_spaces(12));
    state.compose(80, 20).expect("narrow horizontal frame");
    assert!(
        state.hits.spaces_bar.max_scroll > 0,
        "twelve chips should overflow eighty columns"
    );
    assert_eq!(state.workspace_scroll, 0);

    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 20,
        row: 0,
        modifiers: KeyModifiers::empty(),
    })]);

    assert_eq!(
        state.workspace_scroll, 1,
        "the wheel moves the strip sideways rather than a vertical list"
    );
}

#[test]
fn the_bar_offers_no_dividers_to_drag() {
    let mut state = horizontal_state(snapshot_with_spaces(3));
    state.compose(100, 30).expect("horizontal frame");

    assert_eq!(
        state.hits.sidebar_divider,
        Rect::default(),
        "there is no rail edge to drag for width"
    );
    assert_eq!(
        state.hits.sidebar_section_divider,
        Rect::default(),
        "the bar has no spaces/agents split to drag"
    );
    assert_eq!(
        state.hits.workspace_body,
        Rect::default(),
        "an empty workspace body keeps vertical drag-to-reorder inert"
    );
}

#[test]
fn an_untoggled_layout_is_not_persisted_over_config() {
    let mut config = Config::default();
    config.ui.sidebar_layout = SidebarLayoutConfig::Horizontal;
    let state = ClientShellState::new(ClientShellConfig::from_config(&config));

    assert_eq!(state.sidebar_layout, SidebarLayoutConfig::Horizontal);
    assert!(
        !state.sidebar_layout_manual,
        "a config-provided placement is not the client's to persist"
    );
}

#[test]
fn a_persisted_layout_outranks_config() {
    let mut config = Config::default();
    config.ui.sidebar_layout = SidebarLayoutConfig::Vertical;
    let mut shell_config = ClientShellConfig::from_config(&config);
    shell_config.preferences.sidebar_layout = Some(SidebarLayoutConfig::Horizontal);
    let state = ClientShellState::new(shell_config);

    assert_eq!(
        state.sidebar_layout,
        SidebarLayoutConfig::Horizontal,
        "a placement the user toggled survives restart"
    );
    assert!(state.sidebar_layout_manual);
}
