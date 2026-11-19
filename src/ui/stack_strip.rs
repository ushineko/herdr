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

/// Lay out a stack's strip entries left to right in `strip`. Labels are cut to
/// share the width; entries that do not fit are dropped from the right.
pub(crate) fn strip_entries(
    app: &AppState,
    tab: &Tab,
    visible: PaneId,
    strip: Rect,
) -> Vec<StripEntry> {
    let Some(stack) = tab.stack_of(visible) else {
        return Vec::new();
    };
    let count = stack.panes.len() as u16;
    let share = (strip.width / count.max(1)).max(4);
    let mut entries = Vec::new();
    let mut x = strip.x;
    for pane_id in &stack.panes {
        if x >= strip.right() {
            break;
        }
        let width = share.min(strip.right() - x);
        let label = member_label(app, tab, *pane_id, *pane_id != visible);
        let text = format!(
            " {} ",
            crate::ui::truncate_end(&label, usize::from(width.saturating_sub(2)))
        );
        entries.push(StripEntry {
            pane_id: *pane_id,
            x,
            width,
            label: text,
        });
        x += width;
    }
    entries
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
        let palette = &app.palette;
        let buffer = frame.buffer_mut();
        let base = Style::default().fg(palette.overlay0);
        for x in strip.x..strip.right() {
            buffer[(x, strip.y)].set_symbol(" ").set_style(base);
        }
        for entry in strip_entries(app, tab, info.id, strip) {
            let style = if entry.pane_id == info.id {
                let color = if info.id == focused {
                    palette.accent
                } else {
                    palette.text
                };
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                base
            };
            buffer.set_stringn(
                entry.x,
                strip.y,
                &entry.label,
                usize::from(entry.width),
                style,
            );
        }
    }
}

#[cfg(test)]
mod tests {

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
