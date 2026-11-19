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
    horizontal_state_with(snapshot, |_| {})
}

fn horizontal_state_with(
    snapshot: ClientShellSnapshot,
    configure: impl FnOnce(&mut Config),
) -> ClientShellState {
    let mut config = Config::default();
    config.ui.sidebar_layout = SidebarLayoutConfig::Horizontal;
    configure(&mut config);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state
}

/// The same spaces with focus moved to `index`, as the server would report it.
fn focus_space(mut snapshot: ClientShellSnapshot, index: usize) -> ClientShellSnapshot {
    for (position, workspace) in snapshot.workspaces.iter_mut().enumerate() {
        workspace.focused = position == index;
    }
    snapshot.focused_workspace_id = Some(format!("ws_{}", index + 1));
    snapshot
}

fn fg_at(frame: &FrameData, x: u16, y: u16) -> u32 {
    frame.cells[(y * frame.width + x) as usize].fg
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

#[test]
fn scroll_buttons_appear_only_when_the_chips_overflow() {
    let mut roomy = horizontal_state(snapshot_with_spaces(2));
    roomy.compose(120, 20).expect("roomy frame");
    assert_eq!(
        roomy.hits.spaces_bar.scroll_left_rect,
        Rect::default(),
        "chips fit, so the bar must not spend columns on buttons"
    );
    assert_eq!(roomy.hits.spaces_bar.scroll_right_rect, Rect::default());

    let mut crowded = horizontal_state(snapshot_with_spaces(12));
    crowded.compose(80, 20).expect("crowded frame");
    assert!(
        crowded.hits.spaces_bar.scroll_left_rect.width > 0
            && crowded.hits.spaces_bar.scroll_right_rect.width > 0,
        "overflowing chips should offer scroll buttons: {:?}",
        crowded.hits.spaces_bar
    );
}

#[test]
fn scroll_buttons_sit_outside_the_chips_they_scroll() {
    let mut state = horizontal_state(snapshot_with_spaces(12));
    state.compose(80, 20).expect("crowded frame");
    let bar = &state.hits.spaces_bar;
    assert!(bar.scroll_left_rect.width > 0, "expected buttons: {bar:?}");

    for chip in bar.chips.iter().filter(|chip| chip.rect.width > 0) {
        assert!(
            chip.rect.x >= bar.scroll_left_rect.right(),
            "chip {chip:?} overlaps the left button {:?}",
            bar.scroll_left_rect
        );
        assert!(
            chip.rect.right() <= bar.scroll_right_rect.x,
            "chip {chip:?} overlaps the right button {:?}",
            bar.scroll_right_rect
        );
    }
}

#[test]
fn scroll_buttons_are_not_drawn_without_mouse_chrome() {
    let mut state = horizontal_state_with(snapshot_with_spaces(12), |config| {
        config.ui.mouse_capture = false;
    });
    let frame = state.compose(80, 20).expect("crowded frame");

    assert_eq!(state.hits.spaces_bar.scroll_left_rect, Rect::default());
    assert_eq!(state.hits.spaces_bar.scroll_right_rect, Rect::default());
    assert!(
        frame_rows(&frame)[0].contains('…'),
        "without buttons the overflow hint still marks hidden chips"
    );
}

#[test]
fn a_strip_too_narrow_for_buttons_keeps_its_chips() {
    // 3 (`+`) + 6 (`menu`) of chrome leaves an 8-column strip: one short of
    // room for two 3-column buttons and a chip between them. A terminal that
    // narrow is below the default mobile threshold, which would hide the bar.
    let mut state = horizontal_state_with(snapshot_with_spaces(12), |config| {
        config.ui.mobile_width_threshold = 0;
    });
    state.compose(17, 20).expect("narrow frame");

    assert_eq!(
        state.hits.spaces_bar.scroll_left_rect,
        Rect::default(),
        "buttons must not crowd out the chips they scroll"
    );
    assert!(
        state
            .hits
            .spaces_bar
            .chips
            .iter()
            .any(|chip| chip.rect.width > 0),
        "at least one chip should still be drawn: {:?}",
        state.hits.spaces_bar
    );
}

#[test]
fn a_disabled_arrow_is_dimmer_than_an_active_one() {
    let mut state = horizontal_state(snapshot_with_spaces(12));
    let frame = state.compose(80, 20).expect("crowded frame");
    let bar = &state.hits.spaces_bar;
    assert_eq!(bar.scroll, 0, "the strip starts at its left end");

    let (left, right) = (bar.scroll_left_rect, bar.scroll_right_rect);
    assert_eq!(
        frame_rows(&frame)[0].chars().nth(left.x as usize + 1),
        Some('<')
    );
    assert_eq!(
        frame_rows(&frame)[0].chars().nth(right.x as usize + 1),
        Some('>')
    );
    let palette = &state.config.palette;
    assert_eq!(
        fg_at(&frame, left.x + 1, left.y),
        crate::protocol::color_to_u32(palette.overlay0),
        "nothing is hidden to the left, so that arrow is dim"
    );
    assert_eq!(
        fg_at(&frame, right.x + 1, right.y),
        crate::protocol::color_to_u32(palette.overlay1),
        "chips remain to the right, so that arrow is live"
    );
}

#[test]
fn clicking_the_scroll_arrows_moves_the_strip_both_ways() {
    let mut state = horizontal_state(snapshot_with_spaces(12));
    state.compose(80, 20).expect("crowded frame");
    let right = state.hits.spaces_bar.scroll_right_rect;

    click_at(&mut state, right.x + 1, right.y);
    click_at(&mut state, right.x + 1, right.y);
    state.compose(80, 20).expect("scrolled frame");
    assert_eq!(
        state.workspace_scroll, 2,
        "each right click advances one chip"
    );

    let left = state.hits.spaces_bar.scroll_left_rect;
    click_at(&mut state, left.x + 1, left.y);
    state.compose(80, 20).expect("scrolled back frame");
    assert_eq!(state.workspace_scroll, 1, "the left arrow walks it back");
}

#[test]
fn clicking_a_scroll_arrow_does_not_switch_space() {
    let mut state = horizontal_state(snapshot_with_spaces(12));
    state.compose(80, 20).expect("crowded frame");
    let right = state.hits.spaces_bar.scroll_right_rect;

    let press = click_at(&mut state, right.x + 1, right.y);
    let release = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: right.x + 1,
        row: right.y,
        modifiers: KeyModifiers::empty(),
    })]);

    assert!(
        press.actions.is_empty() && release.actions.is_empty(),
        "an arrow click is navigation within the bar, not a space switch"
    );
}

#[test]
fn a_scrolled_strip_survives_the_next_frame() {
    // The first space is focused, so the reveal rule and a rightward scroll
    // disagree: before the fix the next frame dragged the strip back to 0.
    let mut state = horizontal_state(snapshot_with_spaces(12));
    state.compose(80, 20).expect("crowded frame");
    let right = state.hits.spaces_bar.scroll_right_rect;

    click_at(&mut state, right.x + 1, right.y);
    state.compose(80, 20).expect("next frame");

    assert_eq!(
        state.workspace_scroll, 1,
        "a manual scroll was undone by the focus reveal on the next frame"
    );
}

#[test]
fn the_wheel_also_survives_the_next_frame() {
    let mut state = horizontal_state(snapshot_with_spaces(12));
    state.compose(80, 20).expect("crowded frame");

    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 20,
        row: 0,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(80, 20).expect("next frame");

    assert_eq!(
        state.workspace_scroll, 1,
        "the wheel was undone by the focus reveal on the next frame"
    );
}

#[test]
fn a_focus_change_still_reveals_the_newly_focused_chip() {
    let spaces = snapshot_with_spaces(12);
    let mut state = horizontal_state(spaces.clone());
    state.compose(80, 20).expect("crowded frame");
    let right = state.hits.spaces_bar.scroll_right_rect;
    click_at(&mut state, right.x + 1, right.y);
    state.compose(80, 20).expect("user owns the offset");

    state.set_snapshot(Box::new(focus_space(spaces, 11)));
    state.compose(80, 20).expect("frame after focus moves");

    let last = state
        .hits
        .spaces_bar
        .chips
        .iter()
        .find(|chip| chip.index == 11)
        .expect("last chip");
    assert!(
        last.full,
        "the newly focused chip must be pulled fully into view: {:?}",
        state.hits.spaces_bar
    );
}
