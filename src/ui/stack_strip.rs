//! Local patch: pane-stack strip geometry and drawing (spec 002,
//! `ushineko/specs/002-nested-tabs.md`).
//!
//! A stack's strip is the top row of its visible member's content area; the
//! terminal gets the rows below it. Sizing, resizing, rendering and the
//! client's strip geometry all go through `strip_and_content`, so they agree.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use crate::app::AppState;
use crate::layout::{PaneId, PaneInfo};
use crate::workspace::Tab;

/// One entry of a drawn strip: the member and the cells it occupies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StripEntry {
    pub pane_id: PaneId,
    pub x: u16,
    pub width: u16,
    pub label: String,
}

/// Split a pane's content area into its strip row and the terminal's area.
/// Only a stack's visible member has a strip, and only when the area has at
/// least two rows; otherwise the whole area is the terminal's.
pub(crate) fn strip_and_content(
    tab: &Tab,
    pane_id: PaneId,
    pane_inner: Rect,
) -> (Option<Rect>, Rect) {
    let stacked = tab
        .stack_of(pane_id)
        .is_some_and(|stack| stack.visible() == Some(pane_id));
    if !stacked || pane_inner.height < 2 {
        return (None, pane_inner);
    }
    (
        Some(Rect::new(pane_inner.x, pane_inner.y, pane_inner.width, 1)),
        Rect::new(
            pane_inner.x,
            pane_inner.y + 1,
            pane_inner.width,
            pane_inner.height - 1,
        ),
    )
}

/// The terminal's area within a pane's content area.
pub(crate) fn content_rect(tab: &Tab, pane_id: PaneId, pane_inner: Rect) -> Rect {
    strip_and_content(tab, pane_id, pane_inner).1
}

/// A label as the strip shows it: namespaced labels such as `panel:yazi` or
/// `monitor:btop` show only their last part, since the strip already says
/// these panes belong together.
pub(crate) fn strip_label_text(label: &str) -> &str {
    match label.rsplit_once(':') {
        Some((_, name)) if !name.trim().is_empty() => name.trim(),
        _ => label,
    }
}

/// A member's strip label: its manual label, else its agent, else its title,
/// else "shell". A hidden member whose agent is blocked or done is prefixed
/// with that state's glyph.
fn member_label(app: &AppState, tab: &Tab, pane_id: PaneId, hidden: bool) -> String {
    let pane = tab.panes.get(&pane_id);
    let terminal = pane.and_then(|pane| app.terminals.get(&pane.attached_terminal_id));
    let name = terminal
        .and_then(|terminal| {
            terminal
                .manual_label
                .as_deref()
                .map(|label| strip_label_text(label).to_string())
                .or_else(|| terminal.effective_display_agent())
                .or_else(|| terminal.effective_agent_label().map(str::to_string))
                .or_else(|| terminal.effective_title())
        })
        .unwrap_or_else(|| "shell".to_string());
    let glyph = match (
        terminal.map(|terminal| terminal.state),
        pane.map(|pane| pane.seen),
    ) {
        (Some(crate::detect::AgentState::Blocked), _) if hidden => Some("×"),
        (Some(crate::detect::AgentState::Idle), Some(false)) if hidden => Some("✓"),
        _ => None,
    };
    match glyph {
        Some(glyph) => format!("{glyph} {name}"),
        None => name,
    }
}

/// Lay out entries for these labels in `strip`, the way the tab bar sizes its
/// tabs: label plus padding (at least `MIN_ENTRY_WIDTH`), one column between
/// entries. When they do not fit, the width is shared equally instead.
pub(crate) fn layout_entries(labels: Vec<(PaneId, String)>, strip: Rect) -> Vec<StripEntry> {
    const MIN_ENTRY_WIDTH: u16 = 8;
    const MIN_SHARED_WIDTH: u16 = 4;
    let count = labels.len() as u16;
    if count == 0 {
        return Vec::new();
    }
    let gaps = count - 1;
    let desired: Vec<u16> = labels
        .iter()
        .map(|(_, label)| {
            (crate::ui::display_width(label) as u16)
                .saturating_add(4)
                .max(MIN_ENTRY_WIDTH)
        })
        .collect();
    let total = desired
        .iter()
        .fold(gaps, |sum, width| sum.saturating_add(*width));
    let shared = (strip.width.saturating_sub(gaps) / count).max(MIN_SHARED_WIDTH);
    let mut entries = Vec::new();
    let mut x = strip.x;
    for ((pane_id, label), want) in labels.into_iter().zip(desired) {
        if x >= strip.right() {
            break;
        }
        let width = if total <= strip.width { want } else { shared }.min(strip.right() - x);
        entries.push(StripEntry {
            pane_id,
            x,
            width,
            label,
        });
        x = x.saturating_add(width + 1);
    }
    entries
}

/// A workspace stack's strip entries.
pub(crate) fn strip_entries(
    app: &AppState,
    tab: &Tab,
    visible: PaneId,
    strip: Rect,
) -> Vec<StripEntry> {
    let Some(stack) = tab.stack_of(visible) else {
        return Vec::new();
    };
    let labels = stack
        .panes
        .iter()
        .map(|pane_id| {
            (
                *pane_id,
                member_label(app, tab, *pane_id, *pane_id != visible),
            )
        })
        .collect();
    layout_entries(labels, strip)
}

/// Draw a strip in the tab bar's style: the row on `panel_bg`, entries on
/// `surface0`, the shown member on the accent colour while its slot has focus
/// (a raised `surface1` otherwise), labels centred.
pub(crate) fn draw_strip(
    buffer: &mut ratatui::buffer::Buffer,
    strip: Rect,
    entries: &[StripEntry],
    visible: PaneId,
    slot_focused: bool,
    palette: &crate::app::state::Palette,
) {
    buffer.set_style(strip, Style::default().bg(palette.panel_bg));
    for x in strip.x..strip.right() {
        buffer[(x, strip.y)].set_symbol(" ");
    }
    for entry in entries {
        let style = if entry.pane_id == visible && slot_focused {
            Style::default()
                .fg(super::widgets::panel_contrast_fg(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else if entry.pane_id == visible {
            Style::default()
                .fg(palette.text)
                .bg(palette.surface1)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay1).bg(palette.surface0)
        };
        let width = usize::from(entry.width);
        let label = crate::ui::truncate_end(&entry.label, width.saturating_sub(2));
        let padding = width.saturating_sub(crate::ui::display_width(&label));
        let left = padding / 2;
        let text = format!(
            "{empty:left$}{label}{empty:right$}",
            empty = "",
            right = padding - left
        );
        buffer.set_stringn(entry.x, strip.y, &text, width, style);
    }
}

/// Draw the strips of every stack among `pane_infos` (the drawn panes).
pub(super) fn render_stack_strips(
    app: &AppState,
    tab: &Tab,
    pane_infos: &[PaneInfo],
    focused: PaneId,
    frame: &mut Frame,
) {
    for info in pane_infos {
        let pane_inner = super::panes::pane_inner_rect(info.rect, info.borders);
        let (Some(strip), _) = strip_and_content(tab, info.id, pane_inner) else {
            continue;
        };
        let entries = strip_entries(app, tab, info.id, strip);
        draw_strip(
            frame.buffer_mut(),
            strip,
            &entries,
            info.id,
            info.id == focused,
            &app.palette,
        );
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_strip_is_drawn_in_the_tab_bar_style() {
        let palette = crate::app::state::Palette::tokyo_night();
        let strip = Rect::new(0, 0, 60, 1);
        let (yazi, lazygit) = (PaneId::alloc(), PaneId::alloc());
        let entries = layout_entries(
            vec![(yazi, "yazi".into()), (lazygit, "lazygit".into())],
            strip,
        );
        assert_eq!(
            (entries[0].x, entries[0].width),
            (0, 8),
            "label + padding, min 8"
        );
        assert_eq!(
            (entries[1].x, entries[1].width),
            (9, 11),
            "one column between tabs"
        );

        let mut buffer = ratatui::buffer::Buffer::empty(strip);
        draw_strip(&mut buffer, strip, &entries, yazi, true, &palette);
        assert_eq!(
            buffer[(0, 0)].bg,
            palette.accent,
            "the shown tab on the accent"
        );
        assert_eq!(buffer[(9, 0)].bg, palette.surface0, "others on surface0");
        assert_eq!(
            buffer[(8, 0)].bg,
            palette.panel_bg,
            "gaps on the bar background"
        );
        assert_eq!(buffer[(59, 0)].bg, palette.panel_bg);
        let row: String = (0..60)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(row.starts_with("  yazi  "), "centred: {row:?}");

        draw_strip(&mut buffer, strip, &entries, yazi, false, &palette);
        assert_eq!(
            buffer[(0, 0)].bg,
            palette.surface1,
            "an unfocused slot's tab is quieter"
        );

        let narrow = layout_entries(
            vec![
                (yazi, "a-long-label".into()),
                (lazygit, "another-long-label".into()),
            ],
            Rect::new(0, 0, 21, 1),
        );
        assert_eq!(
            narrow.iter().map(|e| e.width).collect::<Vec<_>>(),
            vec![10, 10],
            "shared when tight"
        );
    }

    #[test]
    fn namespaced_labels_show_their_last_part() {
        assert_eq!(strip_label_text("panel:yazi"), "yazi");
        assert_eq!(strip_label_text("monitor:btop"), "btop");
        assert_eq!(strip_label_text("a:b:c"), "c");
        assert_eq!(strip_label_text("plain"), "plain");
        assert_eq!(strip_label_text("trailing:"), "trailing:");
    }

    use super::*;
    use crate::workspace::Workspace;

    #[tokio::test]
    async fn computing_the_active_tab_with_resize_sizes_hidden_members_like_the_visible_one() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("stack-sizes");
        let root = workspace.tabs[0].root_pane;
        let shown = workspace.test_stack(root);
        for pane in [root, shown] {
            workspace.insert_test_runtime(
                pane,
                crate::terminal::TerminalRuntime::test_with_screen_bytes(10, 5, b""),
            );
        }
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app.ensure_test_terminals();
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let infos = super::super::panes::compute_pane_infos_for_tab(
            &app,
            &runtimes,
            0,
            0,
            Rect::new(0, 0, 60, 20),
            true,
            crate::kitty_graphics::HostCellSize::default(),
        );
        assert_eq!(infos.len(), 1, "only the visible member is laid out");
        let size = |pane| {
            app.runtime_for_pane_in_workspace(&runtimes, 0, pane)
                .map(|runtime| runtime.current_size())
                .unwrap()
        };
        assert_eq!(size(root), size(shown));
        assert_eq!(
            size(shown).0,
            infos[0].inner_rect.height,
            "the visible member's rows exclude the strip"
        );
    }
}
