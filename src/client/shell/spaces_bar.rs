//! Experimental horizontal spaces bar.
//!
//! Renders the spaces list as a single-row tab strip across the top of the
//! screen instead of the left rail, folding the agent panel into a compact
//! per-space status rollup. Enabled with `ui.sidebar_layout = "horizontal"`.
//!
//! This is client chrome only: nothing here is exposed through the server API,
//! and the underlying workspace/agent facts in the snapshot are unchanged.

use super::*;

use crate::api::schema::AgentStatus;
use crate::protocol::ClientShellAgent;
use crate::ui::{display_width, truncate_end};

use super::render::{put_text, ShellRenderState};

fn display_width_u16(text: &str) -> u16 {
    display_width(text).min(u16::MAX as usize) as u16
}

/// Rows the spaces bar occupies when it is visible: one row of chips plus a
/// separator rule, mirroring the column the vertical sidebar reserves for `│`.
pub(crate) const SPACES_BAR_HEIGHT: u16 = 2;
/// Width of the leading `+` (new space) hotspot when mouse chrome is on.
const NEW_BUTTON_WIDTH: u16 = 3;
/// Width of the trailing `menu` hotspot when mouse chrome is on.
const MENU_WIDTH: u16 = 6;
/// Extra trailing width when the menu carries an attention badge.
const MENU_BADGE_WIDTH: u16 = 8;
/// Longest space label before it is truncated inside a chip.
const MAX_LABEL_WIDTH: usize = 18;
/// One blank column between chips.
const CHIP_GAP: u16 = 1;
/// Width of each `<` / `>` scroll hotspot, matching the tab bar's buttons.
const SCROLL_BUTTON_WIDTH: u16 = 3;
/// Narrowest chip strip that still has room for two scroll buttons and a chip
/// between them. Below this the buttons would crowd out what they scroll, so
/// the bar falls back to the non-interactive overflow ellipsis.
const MIN_SCROLLABLE_STRIP_WIDTH: u16 = SCROLL_BUTTON_WIDTH * 2 + 3;

/// A rendered space entry in the bar, plus the sub-rect of its status rollup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpacesBarChip {
    pub(super) index: usize,
    pub(super) indented: bool,
    pub(super) rect: Rect,
    /// Columns covered by the agent status glyphs, empty when the space has no agents.
    pub(super) status_rect: Rect,
    /// False when the chip is clipped by the right edge and only partly drawn.
    pub(super) full: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SpacesBarView {
    /// The whole bar, rule included. Empty in vertical placement.
    pub(super) area: Rect,
    pub(super) scroll: usize,
    /// Largest useful scroll, resolved against the same layout the chips used.
    pub(super) max_scroll: usize,
    pub(super) chips: Vec<SpacesBarChip>,
    pub(super) new_rect: Rect,
    pub(super) menu_rect: Rect,
    pub(super) overflow_left: bool,
    pub(super) overflow_right: bool,
    /// `<` hotspot, empty when the chips fit or mouse chrome is off.
    pub(super) scroll_left_rect: Rect,
    /// `>` hotspot, empty when the chips fit or mouse chrome is off.
    pub(super) scroll_right_rect: Rect,
}

/// A folded agent status rollup: one glyph per status class, with counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StatusCount {
    status: AgentStatus,
    count: usize,
}

/// Workspace indices whose agents roll up into one chip. A collapsed worktree
/// group shows its members' agents on the parent chip so nothing goes missing
/// when the group is folded.
fn rollup_indices(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
    entry: &WorkspaceEntry,
) -> Vec<usize> {
    let collapsed_key = (!entry.indented)
        .then(|| super::sidebar::parent_group_key(snapshot, entry.index))
        .flatten()
        .filter(|key| collapsed_groups.contains(key));

    let Some(key) = collapsed_key else {
        return vec![entry.index];
    };

    snapshot
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, workspace)| {
            workspace
                .worktree
                .as_ref()
                .is_some_and(|worktree| worktree.key == key)
        })
        .map(|(index, _)| index)
        .collect()
}

/// Fold every agent pane in `indices` into at most three status classes,
/// ordered by how much attention they want.
fn status_counts(snapshot: &ClientShellSnapshot, indices: &[usize]) -> Vec<StatusCount> {
    let workspace_ids = indices
        .iter()
        .filter_map(|index| snapshot.workspaces.get(*index))
        .map(|workspace| workspace.workspace_id.as_str())
        .collect::<HashSet<_>>();

    let mut counts: Vec<StatusCount> = Vec::new();
    for agent in snapshot
        .agents
        .iter()
        .filter(|agent| workspace_ids.contains(agent.workspace_id.as_str()))
    {
        match counts
            .iter_mut()
            .find(|count| count.status == agent.agent_status)
        {
            Some(existing) => existing.count += 1,
            None => counts.push(StatusCount {
                status: agent.agent_status,
                count: 1,
            }),
        }
    }

    counts.sort_by(|left, right| {
        super::status_priority(right.status)
            .cmp(&super::status_priority(left.status))
            .then(right.count.cmp(&left.count))
    });
    counts.truncate(3);
    counts
}

/// `◐2 ×1` — one glyph per status class, suffixed with its count when it folds
/// more than one agent.
fn status_spans(
    counts: &[StatusCount],
    indicators: crate::config::StatusIndicatorStyle,
) -> Vec<(String, AgentStatus)> {
    counts
        .iter()
        .map(|count| {
            let icon = super::status_icon(count.status, indicators);
            let text = if count.count > 1 {
                format!("{icon}{}", count.count)
            } else {
                icon.to_string()
            };
            (text, count.status)
        })
        .collect()
}

fn chip_label(snapshot: &ClientShellSnapshot, entry: &WorkspaceEntry) -> String {
    let Some(workspace) = snapshot.workspaces.get(entry.index) else {
        return String::new();
    };
    // Mirrors the vertical rail: a grouped child is named by its branch, since
    // every sibling in the group repeats the same workspace label.
    let label = if entry.indented && !workspace.custom_label {
        workspace
            .branch
            .as_deref()
            .and_then(|branch| branch.strip_prefix("worktree/").or(Some(branch)))
            .unwrap_or(&workspace.label)
    } else {
        &workspace.label
    };
    truncate_end(label, MAX_LABEL_WIDTH)
}

/// The status glyph shown before a chip label. A collapsed group parent shows
/// the whole group's aggregate so a folded group still signals attention.
fn chip_status(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
    entry: &WorkspaceEntry,
) -> AgentStatus {
    snapshot
        .workspaces
        .get(entry.index)
        .map(|workspace| {
            super::sidebar::displayed_workspace_status(snapshot, workspace, collapsed_groups)
        })
        .unwrap_or(AgentStatus::Unknown)
}

struct ChipContent {
    number: Option<String>,
    prefix: Option<&'static str>,
    status: AgentStatus,
    label: String,
    rollup: Vec<(String, AgentStatus)>,
}

impl ChipContent {
    /// ` 1 ●label ◐2 ×1 ` — a leading and trailing pad keep chips readable
    /// when they sit shoulder to shoulder with a highlighted neighbor.
    fn width(&self, indicators: crate::config::StatusIndicatorStyle) -> u16 {
        let mut width = 2; // outer padding
        if let Some(number) = &self.number {
            width += display_width_u16(number) + 1;
        }
        if let Some(prefix) = self.prefix {
            width += display_width_u16(prefix);
        }
        width += display_width_u16(super::status_icon(self.status, indicators));
        if !self.label.is_empty() {
            // One space after the status icon, matching the vertical sidebar.
            width += 1 + display_width_u16(&self.label);
        }
        for (text, _) in &self.rollup {
            width += display_width_u16(text) + 1;
        }
        width
    }
}

fn chip_content(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_groups: &HashSet<String>,
    entry: &WorkspaceEntry,
) -> ChipContent {
    let number = (entry.index < 9).then(|| (entry.index + 1).to_string());
    let rollup = rollup_indices(snapshot, collapsed_groups, entry);
    ChipContent {
        number,
        prefix: entry.indented.then_some("↳"),
        status: chip_status(snapshot, collapsed_groups, entry),
        label: chip_label(snapshot, entry),
        rollup: status_spans(&status_counts(snapshot, &rollup), config.status_indicators),
    }
}

fn menu_width(snapshot: &ClientShellSnapshot) -> u16 {
    if super::global_menu::global_menu_attention(snapshot) {
        MENU_BADGE_WIDTH
    } else {
        MENU_WIDTH
    }
}

/// The chip row, excluding the separator rule on the bar's last row. Degrades to
/// the whole area when the bar is squeezed down to a single row.
fn bar_content_rect(area: Rect) -> Rect {
    if area.height <= 1 {
        return area;
    }
    Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
}

/// The separator rule between the bar and everything below it.
fn bar_separator_rect(area: Rect) -> Rect {
    if area.height <= 1 || area.width == 0 {
        return Rect::default();
    }
    Rect::new(
        area.x,
        area.y + area.height.saturating_sub(1),
        area.width,
        1,
    )
}

/// Colour of the rule under the bar.
///
/// `overlay0` is what unfocused pane borders use, and it is the only muted token
/// with real contrast in every built-in theme. `surface_dim` — which the vertical
/// sidebar uses for its `│` — is a *fill* colour: it is exactly `panel_bg` in 8 of
/// the 18 built-in themes (tokyo-night, dracula, nord, gruvbox, one-dark,
/// solarized, kanagawa, rose-pine), so a rule drawn in it disappears.
fn separator_style(palette: &Palette, is_navigating: bool) -> Style {
    if is_navigating {
        Style::default().fg(palette.accent)
    } else {
        Style::default().fg(palette.overlay0)
    }
}

/// Left/right chrome hotspots, and the strip left for chips between them.
fn bar_sections(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    full_area: Rect,
) -> (Rect, Rect, Rect) {
    let area = bar_content_rect(full_area);
    if area.width == 0 || area.height == 0 {
        return (Rect::default(), Rect::default(), Rect::default());
    }
    if !config.mouse_capture {
        return (Rect::default(), Rect::default(), area);
    }

    let menu_w = menu_width(snapshot).min(area.width);
    let new_w = NEW_BUTTON_WIDTH.min(area.width.saturating_sub(menu_w));
    let new_rect = Rect::new(area.x, area.y, new_w, 1);
    let menu_rect = Rect::new(
        area.x + area.width.saturating_sub(menu_w),
        area.y,
        menu_w,
        1,
    );
    let chips = Rect::new(
        area.x + new_w,
        area.y,
        area.width.saturating_sub(new_w + menu_w),
        1,
    );
    (new_rect, menu_rect, chips)
}

fn layout_chips(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_groups: &HashSet<String>,
    entries: &[WorkspaceEntry],
    area: Rect,
    scroll: usize,
) -> Vec<SpacesBarChip> {
    let mut chips = entries
        .iter()
        .map(|entry| SpacesBarChip {
            index: entry.index,
            indented: entry.indented,
            rect: Rect::default(),
            status_rect: Rect::default(),
            full: false,
        })
        .collect::<Vec<_>>();
    if area.width == 0 || area.height == 0 {
        return chips;
    }

    let mut x = area.x;
    let right = area.x + area.width;
    for (idx, entry) in entries.iter().enumerate().skip(scroll) {
        if x >= right {
            break;
        }
        let content = chip_content(snapshot, config, collapsed_groups, entry);
        let desired = content.width(config.status_indicators);
        let width = desired.min(right.saturating_sub(x));
        if width < 3 {
            break;
        }
        let status_width = content
            .rollup
            .iter()
            .map(|(text, _)| display_width_u16(text) + 1)
            .sum::<u16>();
        let status_rect = if status_width > 0 && width == desired {
            Rect::new(
                x + width.saturating_sub(status_width + 1),
                area.y,
                status_width,
                1,
            )
        } else {
            Rect::default()
        };
        chips[idx].rect = Rect::new(x, area.y, width, 1);
        chips[idx].status_rect = status_rect;
        chips[idx].full = width == desired;
        x = x.saturating_add(width).saturating_add(CHIP_GAP);
    }
    chips
}

/// Smallest scroll that keeps `target` fully visible, preferring to keep the
/// leftmost chips in view.
fn scroll_revealing(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_groups: &HashSet<String>,
    entries: &[WorkspaceEntry],
    area: Rect,
    target: usize,
    current: usize,
) -> usize {
    // A chip clipped by the right edge does not count as revealed, otherwise
    // scrolling would stop while the focused space is still cut in half.
    let fits = |scroll: usize| {
        layout_chips(snapshot, config, collapsed_groups, entries, area, scroll)
            .get(target)
            .is_some_and(|chip| chip.full)
    };

    if current <= target && fits(current) {
        return current;
    }
    if current > target {
        return (0..=target).find(|scroll| fits(*scroll)).unwrap_or(target);
    }
    (current..=target)
        .find(|scroll| fits(*scroll))
        .unwrap_or(target)
}

/// Smallest scroll that still shows the final chip in full. Scrolling past this
/// would only open dead space on the right, so it caps both the wheel and any
/// stale offset carried over from a previous layout.
fn max_scroll_for(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_groups: &HashSet<String>,
    entries: &[WorkspaceEntry],
    chip_area: Rect,
) -> usize {
    if entries.is_empty() {
        return 0;
    }
    (0..entries.len())
        .find(|scroll| {
            layout_chips(
                snapshot,
                config,
                collapsed_groups,
                entries,
                chip_area,
                *scroll,
            )
            .last()
            .is_some_and(|chip| chip.full)
        })
        .unwrap_or(entries.len().saturating_sub(1))
}

/// Whether the chips need more room than `area` gives them.
///
/// Defined against the *unreserved* strip, so the answer does not depend on the
/// buttons it decides. Reserving the buttons only ever narrows the strip, which
/// cannot turn an overflowing bar back into a fitting one — so this settles in
/// one pass rather than oscillating.
fn chips_overflow(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_groups: &HashSet<String>,
    entries: &[WorkspaceEntry],
    area: Rect,
) -> bool {
    !layout_chips(snapshot, config, collapsed_groups, entries, area, 0)
        .last()
        .is_some_and(|chip| chip.full)
}

/// Resolve the chip strip for `area`.
///
/// When `reveal` is set the strip scrolls to show `focus_index` — the
/// navigate-mode selection when navigating, otherwise the focused space.
/// Otherwise `workspace_scroll` is kept as the user left it, only clamped to
/// the layout: revealing on every frame would undo each wheel notch and button
/// click on the very next render.
pub(super) fn compute_spaces_bar_view(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_groups: &HashSet<String>,
    area: Rect,
    focus_index: Option<usize>,
    workspace_scroll: usize,
    reveal: bool,
) -> SpacesBarView {
    if area.width == 0 || area.height == 0 {
        return SpacesBarView::default();
    }

    let entries = super::sidebar::workspace_entries(snapshot, collapsed_groups);
    let (new_rect, menu_rect, chip_area) = bar_sections(snapshot, config, area);
    if entries.is_empty() || chip_area.width == 0 {
        return SpacesBarView {
            area,
            new_rect,
            menu_rect,
            ..SpacesBarView::default()
        };
    }

    // Scroll buttons are only worth their columns once the chips actually
    // overflow, and only when clicks can reach them at all.
    let scrollable = config.mouse_capture
        && chip_area.width >= MIN_SCROLLABLE_STRIP_WIDTH
        && chips_overflow(snapshot, config, collapsed_groups, &entries, chip_area);
    let (scroll_left_rect, scroll_right_rect, chip_area) = if scrollable {
        (
            Rect::new(chip_area.x, chip_area.y, SCROLL_BUTTON_WIDTH, 1),
            Rect::new(
                chip_area.right() - SCROLL_BUTTON_WIDTH,
                chip_area.y,
                SCROLL_BUTTON_WIDTH,
                1,
            ),
            Rect::new(
                chip_area.x + SCROLL_BUTTON_WIDTH,
                chip_area.y,
                chip_area.width - SCROLL_BUTTON_WIDTH * 2,
                chip_area.height,
            ),
        )
    } else {
        (Rect::default(), Rect::default(), chip_area)
    };

    let max_scroll = max_scroll_for(snapshot, config, collapsed_groups, &entries, chip_area);
    let scroll = if reveal {
        let focus_entry = focus_index
            .and_then(|index| entries.iter().position(|entry| entry.index == index))
            .unwrap_or(0);
        // Re-resolve from the left so the strip settles on the smallest scroll
        // that shows the focused chip, rather than ratcheting ever rightwards.
        scroll_revealing(
            snapshot,
            config,
            collapsed_groups,
            &entries,
            chip_area,
            focus_entry,
            0,
        )
    } else {
        workspace_scroll.min(max_scroll)
    };

    let chips = layout_chips(
        snapshot,
        config,
        collapsed_groups,
        &entries,
        chip_area,
        scroll,
    );
    let last_visible = chips.iter().rposition(|chip| chip.rect.width > 0);
    SpacesBarView {
        area,
        scroll,
        max_scroll,
        overflow_left: scroll > 0,
        overflow_right: last_visible.is_none_or(|idx| idx + 1 < chips.len()),
        chips,
        new_rect,
        menu_rect,
        scroll_left_rect,
        scroll_right_rect,
    }
}

impl ClientShellState {
    /// Move the strip one chip left or right. The wheel and the `<`/`>` buttons
    /// both land here, so they share the clamp against the last full chip.
    pub(super) fn scroll_spaces_bar(&mut self, forward: bool, outcome: &mut ClientShellInput) {
        let next = if forward {
            self.workspace_scroll
                .saturating_add(1)
                .min(self.hits.spaces_bar.max_scroll)
        } else {
            self.workspace_scroll.saturating_sub(1)
        };
        if next != self.workspace_scroll {
            self.workspace_scroll = next;
            outcome.repaint = true;
        }
    }
}

/// One row of the picker opened by clicking a chip's folded status glyphs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpacesBarAgentEntry {
    pub(super) workspace_index: usize,
    pub(super) pane_id: String,
    pub(super) status: AgentStatus,
    pub(super) title: String,
    pub(super) status_label: String,
}

/// Every agent folded into one chip's status rollup, most in need of attention
/// first — the same order the glyphs are drawn in. Titles are padded so the rows
/// line up when they are listed in the picker.
pub(super) fn status_agents(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
    workspace_index: usize,
) -> Vec<SpacesBarAgentEntry> {
    let entry = WorkspaceEntry {
        index: workspace_index,
        indented: false,
        last_child: false,
    };
    let rollup = rollup_indices(snapshot, collapsed_groups, &entry);
    let workspace_index_of = |workspace_id: &str| {
        snapshot
            .workspaces
            .iter()
            .position(|workspace| workspace.workspace_id == workspace_id)
    };
    let rolled_ids = rollup
        .iter()
        .filter_map(|index| snapshot.workspaces.get(*index))
        .map(|workspace| workspace.workspace_id.as_str())
        .collect::<HashSet<_>>();

    let mut agents = snapshot
        .agents
        .iter()
        .filter(|agent| rolled_ids.contains(agent.workspace_id.as_str()))
        .collect::<Vec<_>>();
    agents.sort_by_key(|agent| std::cmp::Reverse(super::status_priority(agent.agent_status)));

    let name_of = |agent: &ClientShellAgent| {
        agent
            .display_agent
            .clone()
            .or_else(|| agent.agent.clone())
            .or_else(|| agent.name.clone())
            .unwrap_or_else(|| "agent".to_string())
    };
    let context_of = |agent: &ClientShellAgent| {
        agent
            .title
            .clone()
            .or_else(|| agent.terminal_title_stripped.clone())
            .or_else(|| agent.terminal_title.clone())
            .unwrap_or_default()
    };
    let name_width = agents
        .iter()
        .map(|agent| display_width(&name_of(agent)))
        .max()
        .unwrap_or(0);

    let titles = agents
        .iter()
        .map(|agent| {
            let name = name_of(agent);
            let pad = " ".repeat(name_width.saturating_sub(display_width(&name)));
            let context = context_of(agent);
            if context.is_empty() {
                format!("{name}{pad}")
            } else {
                format!("{name}{pad} · {context}")
            }
        })
        .collect::<Vec<_>>();
    // Pad to a common width so the status column lines up down the picker.
    let title_width = titles
        .iter()
        .map(|title| display_width(title))
        .max()
        .unwrap_or(0);

    agents
        .iter()
        .zip(titles)
        .map(|(agent, title)| {
            let title = format!(
                "{title}{}",
                " ".repeat(title_width.saturating_sub(display_width(&title)))
            );
            SpacesBarAgentEntry {
                workspace_index: workspace_index_of(&agent.workspace_id).unwrap_or(workspace_index),
                pane_id: agent.pane_id.clone(),
                status: agent.agent_status,
                title,
                status_label: super::status_text(agent.agent_status).to_string(),
            }
        })
        .collect()
}

pub(super) fn render_spaces_bar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    if area.width == 0 || area.height == 0 {
        hits.spaces_bar = SpacesBarView::default();
        return;
    }

    let palette = &config.palette;
    let is_navigating = state.selected_workspace_id.is_some();
    let content = bar_content_rect(area);
    for y in content.y..content.y + content.height {
        for x in content.x..content.x + content.width {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_char(' ')
                    .set_style(Style::default().bg(palette.panel_bg));
            }
        }
    }

    let separator = bar_separator_rect(area);
    if separator.width > 0 {
        let style = separator_style(palette, is_navigating);
        for x in separator.x..separator.x + separator.width {
            if let Some(cell) = buffer.cell_mut((x, separator.y)) {
                cell.set_symbol("─").set_style(style);
            }
        }
    }

    let entries = super::sidebar::workspace_entries(snapshot, state.collapsed_groups);
    let focus_index = if is_navigating {
        entries.iter().map(|entry| entry.index).find(|index| {
            snapshot.workspaces.get(*index).is_some_and(|workspace| {
                state.selected_workspace_id.is_some_and(|target| {
                    target.matches(state.active_endpoint_id, &workspace.workspace_id)
                })
            })
        })
    } else {
        entries
            .iter()
            .map(|entry| entry.index)
            .find(|index| snapshot.workspaces.get(*index).is_some_and(|ws| ws.focused))
    };

    // Both flags must be consumed, not short-circuited: leaving one set would
    // re-trigger a reveal on the next frame, or strand it for the vertical rail.
    let reveal_focused = std::mem::take(state.reveal_focused_workspace);
    let reveal_navigation = std::mem::take(state.reveal_navigation_workspace);

    let view = compute_spaces_bar_view(
        snapshot,
        config,
        state.collapsed_groups,
        area,
        focus_index,
        *state.workspace_scroll,
        reveal_focused || reveal_navigation,
    );
    *state.workspace_scroll = view.scroll;

    for (idx, entry) in entries.iter().enumerate() {
        let Some(chip) = view.chips.get(idx).filter(|chip| chip.rect.width > 0) else {
            continue;
        };
        let Some(workspace) = snapshot.workspaces.get(entry.index) else {
            continue;
        };
        let selected = is_navigating
            && state.selected_workspace_id.is_some_and(|target| {
                target.matches(state.active_endpoint_id, &workspace.workspace_id)
            });
        let is_active = workspace.focused;
        let chip_bg = if is_active {
            palette.accent
        } else if selected {
            palette.surface1
        } else {
            palette.surface0
        };
        let label_style = if is_active {
            Style::default()
                .fg(super::panel_contrast_fg(palette))
                .bg(chip_bg)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(palette.text)
                .bg(chip_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay1).bg(chip_bg)
        };
        let dim_style = if is_active {
            Style::default()
                .fg(super::panel_contrast_fg(palette))
                .bg(chip_bg)
        } else {
            Style::default().fg(palette.overlay0).bg(chip_bg)
        };

        let content = chip_content(snapshot, config, state.collapsed_groups, entry);
        let mut x = chip.rect.x;
        let right = chip.rect.x + chip.rect.width;
        let put = |buffer: &mut Buffer, x: &mut u16, text: &str, style: Style| {
            for character in text.chars() {
                if *x >= right {
                    return;
                }
                if let Some(cell) = buffer.cell_mut((*x, chip.rect.y)) {
                    cell.set_char(character).set_style(style);
                }
                *x += 1;
            }
        };

        put(buffer, &mut x, " ", label_style);
        if let Some(number) = &content.number {
            put(buffer, &mut x, number, dim_style);
            put(buffer, &mut x, " ", dim_style);
        }
        if let Some(prefix) = content.prefix {
            put(buffer, &mut x, prefix, dim_style);
        }
        put(
            buffer,
            &mut x,
            super::status_icon(content.status, config.status_indicators),
            Style::default()
                .fg(super::status_color(content.status, palette))
                .bg(chip_bg),
        );
        if !content.label.is_empty() {
            put(buffer, &mut x, " ", label_style);
            put(buffer, &mut x, &content.label, label_style);
        }
        for (text, status) in &content.rollup {
            put(buffer, &mut x, " ", label_style);
            put(
                buffer,
                &mut x,
                text,
                Style::default()
                    .fg(super::status_color(*status, palette))
                    .bg(chip_bg),
            );
        }
        // Pad out any remaining columns so the chip reads as one solid block.
        while x < right {
            put(buffer, &mut x, " ", label_style);
        }
    }

    if config.mouse_capture {
        if view.new_rect.width > 0 {
            put_text(
                buffer,
                view.new_rect.x,
                view.new_rect.y,
                view.new_rect.width,
                " + ",
                Style::default().fg(palette.overlay0),
            );
        }
        if view.menu_rect.width > 0 {
            if super::global_menu::global_menu_attention(snapshot) {
                put_text(
                    buffer,
                    view.menu_rect.x,
                    view.menu_rect.y,
                    3,
                    " ● ",
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::BOLD),
                );
                put_text(
                    buffer,
                    view.menu_rect.x + 3,
                    view.menu_rect.y,
                    view.menu_rect.width.saturating_sub(3),
                    "menu",
                    Style::default().fg(palette.overlay0),
                );
            } else {
                put_text(
                    buffer,
                    view.menu_rect.x,
                    view.menu_rect.y,
                    view.menu_rect.width,
                    " menu",
                    Style::default().fg(palette.overlay0),
                );
            }
        }
    }

    if view.scroll_left_rect.width > 0 {
        // Enabled/disabled colouring matches the tab bar's scroll buttons, so
        // "dimmer means you are already at the end" reads the same in both bars.
        let arrow = |can_scroll: bool| {
            Style::default().fg(if can_scroll {
                palette.overlay1
            } else {
                palette.overlay0
            })
        };
        for (rect, glyph, can_scroll) in [
            (view.scroll_left_rect, " < ", view.overflow_left),
            (view.scroll_right_rect, " > ", view.overflow_right),
        ] {
            put_text(buffer, rect.x, rect.y, rect.width, glyph, arrow(can_scroll));
        }
    } else {
        // No buttons (mouse chrome off, or too narrow for them): fall back to a
        // static hint so hidden chips are still discoverable.
        let (_, _, chip_area) = bar_sections(snapshot, config, area);
        if chip_area.width > 0 {
            let overflow_style = Style::default().fg(palette.overlay0);
            if view.overflow_left {
                if let Some(cell) = buffer.cell_mut((chip_area.x, chip_area.y)) {
                    cell.set_symbol("…").set_style(overflow_style);
                }
            }
            if view.overflow_right {
                let x = chip_area.x + chip_area.width.saturating_sub(1);
                if let Some(cell) = buffer.cell_mut((x, chip_area.y)) {
                    cell.set_symbol("…").set_style(overflow_style);
                }
            }
        }
    }

    // Chips reuse the vertical rail's hit path, so click-to-focus, context
    // menus and the space context actions all work unchanged. `workspace_body`
    // stays empty: drag-to-reorder resolves a drop row from vertical geometry
    // that a one-row strip does not have.
    for chip in view.chips.iter().filter(|chip| chip.rect.width > 0) {
        let Some(workspace) = snapshot.workspaces.get(chip.index) else {
            continue;
        };
        hits.workspaces.push(WorkspaceHit {
            rect: chip.rect,
            endpoint_id: state.active_endpoint_id.clone(),
            workspace_id: workspace.workspace_id.clone(),
            indented: chip.indented,
            group_toggle: None,
        });
    }
    if config.mouse_capture {
        hits.new_workspace = view.new_rect;
        hits.global_launcher = view.menu_rect;
    }

    hits.spaces_bar = view;
}
