//! Local patch: the dock, a pane area at the bottom of the screen that belongs
//! to no workspace (spec 001, ushineko/specs/001-dock.md).
//!
//! The dock owns a `TileLayout` like a tab does, but lives beside the
//! workspaces, as the popup does. Nothing that enumerates
//! `AppState::workspaces` sees it, so it stays out of the sidebar, the API's
//! pane and workspace lists, agent lists and numbering by construction.
//!
//! Lifecycle, as decided in the spec:
//! - Config seeds the dock only when no saved dock exists; afterwards the
//!   runtime layout (splits, ratios, height, closes) is saved and wins.
//! - A pane from the seed runs its `command` and is respawned with backoff
//!   when it exits. A pane made by a split runs the user's shell and closes
//!   when it exits, like any pane.
//! - Closing the last pane leaves a closed, zero-row dock; reset rebuilds it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::{Direction, Rect};

use crate::config::DockConfig;
use crate::layout::{NavDirection, PaneId, TileLayout};
use crate::terminal::TerminalId;

pub(crate) const RESPAWN_INITIAL_DELAY: Duration = Duration::from_secs(1);
pub(crate) const RESPAWN_MAX_DELAY: Duration = Duration::from_secs(30);
/// A program that stayed up this long starts its backoff over.
pub(crate) const RESPAWN_RESET_AFTER: Duration = Duration::from_secs(60);
/// Width used to size dock ptys before any client reports one.
pub(crate) const DEFAULT_DOCK_WIDTH: u16 = 80;
/// Upper bounds on client-supplied sizes, so a bad value cannot make the
/// dock ptys allocate an enormous screen.
pub(crate) const MAX_DOCK_ROWS: u16 = 200;
pub(crate) const MAX_DOCK_WIDTH: u16 = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DockPaneOrigin {
    /// From `[[ui.dock.panes]]`: runs `command`, respawned when it exits.
    Seeded { label: String, command: String },
    /// Made by a split: runs the default shell, closes when it exits.
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DockPane {
    pub origin: DockPaneOrigin,
    pub cwd: PathBuf,
    /// Live terminal, or `None` while a seeded program waits to respawn.
    pub terminal_id: Option<TerminalId>,
    pub started_at: Option<Instant>,
    /// Delay applied to the next respawn.
    pub backoff: Duration,
    pub respawn_at: Option<Instant>,
}

impl DockPane {
    fn new(origin: DockPaneOrigin, cwd: PathBuf) -> Self {
        Self {
            origin,
            cwd,
            terminal_id: None,
            started_at: None,
            backoff: RESPAWN_INITIAL_DELAY,
            respawn_at: None,
        }
    }

    pub(crate) fn label(&self) -> Option<&str> {
        match &self.origin {
            DockPaneOrigin::Seeded { label, .. } => Some(label),
            DockPaneOrigin::User => None,
        }
    }
}

pub(crate) struct DockLayout {
    pub layout: TileLayout,
    pub panes: HashMap<PaneId, DockPane>,
    pub zoomed: bool,
    /// Pane stacks in the dock (spec 002). Hidden members are in `panes`.
    pub stacks: Vec<crate::workspace::PaneStack>,
}

pub(crate) enum DockState {
    /// No dock has been built or restored; a valid config seed may build one.
    Unbuilt,
    /// The user closed every dock pane. Saved, and only reset rebuilds it.
    Closed,
    Open(DockLayout),
}

/// Everything the server keeps about the dock.
pub(crate) struct Dock {
    pub state: DockState,
    /// Content rows, shared by every client.
    pub rows: u16,
    /// Rows were changed at runtime, so a reseed keeps config's value out.
    pub rows_manual: bool,
    /// Width the dock ptys are sized to, as last reported by a client.
    pub width: u16,
    /// The config seed currently in effect, kept for reset and late seeding.
    pub seed: DockConfig,
    /// Panes closed on purpose whose runtime exit is still in flight.
    retired: HashSet<PaneId>,
    /// The drawn dock is out of date for a reason other than pty output, which
    /// the render loop tracks itself. Full renders redraw the dock only then.
    pub needs_redraw: bool,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            state: DockState::Unbuilt,
            rows: crate::config::DockConfig::default().rows,
            rows_manual: false,
            width: DEFAULT_DOCK_WIDTH,
            seed: DockConfig::default(),
            retired: HashSet::new(),
            needs_redraw: false,
        }
    }
}

/// Where one dock pane is drawn: `rect` includes the separator cells on its
/// right and bottom edges, `inner` is the program's area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockPaneGeometry {
    pub id: PaneId,
    pub rect: Rect,
    pub inner: Rect,
    pub focused: bool,
    /// The row above `inner` holding a stack's strip (spec 002).
    pub strip: Option<Rect>,
}

/// Public id of a dock pane in dock controls and surfaces.
pub(crate) fn dock_pane_public_id(id: PaneId) -> String {
    format!("dock_{}", id.raw())
}

pub(crate) fn parse_dock_pane_id(id: &str) -> Option<PaneId> {
    id.strip_prefix("dock_")?
        .parse::<u32>()
        .ok()
        .map(PaneId::from_raw)
}

/// A pane that needs a process started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnRequest {
    pub pane_id: PaneId,
    pub cwd: PathBuf,
    /// `Some` for a seeded command, `None` for the default shell.
    pub command: Option<String>,
    pub rows: u16,
    pub cols: u16,
}

/// What an exited dock pane turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DockExit {
    /// Not a dock pane.
    NotDock,
    /// A pane closed on purpose; nothing left to do.
    Retired,
    /// A seeded program will be respawned at the given time.
    Respawning(Instant),
    /// A user pane closed with its program.
    Closed,
}

pub(crate) fn expand_cwd(cwd: Option<&std::path::Path>) -> PathBuf {
    match cwd {
        Some(path) => crate::worktree::expand_tilde_path(&path.to_string_lossy()),
        None => crate::worktree::home_dir().unwrap_or_else(|| PathBuf::from("/")),
    }
}

impl Dock {
    pub(crate) fn open(&self) -> Option<&DockLayout> {
        match &self.state {
            DockState::Open(dock) => Some(dock),
            _ => None,
        }
    }

    fn open_mut(&mut self) -> Option<&mut DockLayout> {
        match &mut self.state {
            DockState::Open(dock) => Some(dock),
            _ => None,
        }
    }

    pub(crate) fn contains(&self, pane_id: PaneId) -> bool {
        self.open()
            .is_some_and(|dock| dock.panes.contains_key(&pane_id))
    }

    /// The area the dock's panes are laid out in.
    pub(crate) fn area(&self) -> Rect {
        Rect::new(0, 0, self.width.max(1), self.rows.max(1))
    }

    /// Visible panes and where they are drawn. A pane not on the area's right
    /// or bottom edge gives up its last column or row to a separator. Zoomed,
    /// only the focused pane is visible and it fills the area.
    pub(crate) fn geometry(&self) -> Vec<DockPaneGeometry> {
        let Some(dock) = self.open() else {
            return Vec::new();
        };
        let area = self.area();
        let focused = dock.layout.focused();
        if dock.zoomed {
            let (strip, inner) = stack_strip(dock, focused, area);
            return vec![DockPaneGeometry {
                id: focused,
                rect: area,
                inner,
                focused: true,
                strip,
            }];
        }
        let mut panes = dock
            .layout
            .panes(area)
            .into_iter()
            .map(|info| {
                let rect = info.rect;
                let mut inner = rect;
                if rect.right() < area.right() && inner.width > 1 {
                    inner.width -= 1;
                }
                if rect.bottom() < area.bottom() && inner.height > 1 {
                    inner.height -= 1;
                }
                let (strip, inner) = stack_strip(dock, info.id, inner);
                DockPaneGeometry {
                    id: info.id,
                    rect,
                    inner,
                    focused: info.id == focused,
                    strip,
                }
            })
            .collect::<Vec<_>>();
        panes.sort_by_key(|pane| (pane.rect.y, pane.rect.x));
        panes
    }

    /// Pty size (rows, cols) for each dock pane. Panes hidden by zoom keep the
    /// full dock size, so unzooming does not reflow them twice.
    pub(crate) fn pane_sizes(&self) -> HashMap<PaneId, (u16, u16)> {
        let Some(dock) = self.open() else {
            return HashMap::new();
        };
        let area = self.area();
        if dock.zoomed {
            return dock
                .panes
                .keys()
                .map(|id| (*id, (area.height, area.width)))
                .collect();
        }
        let mut sizes: HashMap<PaneId, (u16, u16)> = self
            .geometry()
            .into_iter()
            .map(|pane| (pane.id, (pane.inner.height.max(1), pane.inner.width.max(1))))
            .collect();
        // Spec 002: hidden stack members are sized like their visible member.
        for stack in &dock.stacks {
            let Some(size) = stack
                .visible()
                .and_then(|visible| sizes.get(&visible).copied())
            else {
                continue;
            };
            for pane_id in &stack.panes {
                sizes.entry(*pane_id).or_insert(size);
            }
        }
        sizes
    }

    /// Move focus to the neighbouring dock pane, stopping at the dock's edges.
    pub(crate) fn focus_direction(&mut self, nav: NavDirection) -> bool {
        let area = self.area();
        let Some(dock) = self.open_mut() else {
            return false;
        };
        if dock.zoomed {
            return false;
        }
        let infos = dock.layout.panes(area);
        let focused = dock.layout.focused();
        let Some(current) = infos.iter().find(|info| info.id == focused) else {
            return false;
        };
        let Some(next) = crate::layout::find_in_direction(current, nav, &infos) else {
            return false;
        };
        dock.layout.focus_pane(next);
        true
    }

    /// Whether the focused pane touches the dock's top edge, where resize mode
    /// moves the dock's height instead of a divider.
    pub(crate) fn focused_touches_top(&self) -> bool {
        self.geometry()
            .iter()
            .find(|pane| pane.focused)
            .is_some_and(|pane| pane.rect.y == 0)
    }

    fn size_of(&self, pane_id: PaneId) -> (u16, u16) {
        self.pane_sizes()
            .get(&pane_id)
            .copied()
            .unwrap_or((self.rows.max(1), self.width.max(1)))
    }

    /// Build the dock from the config seed: one row of panes, left to right,
    /// with ratios matching the width weights. Returns the processes to start.
    pub(crate) fn build_from_seed(&mut self) -> Vec<SpawnRequest> {
        let Some(seed) = self.seed.seed().cloned() else {
            self.state = DockState::Unbuilt;
            return Vec::new();
        };
        let (mut layout, root) = TileLayout::new();
        let mut panes = HashMap::new();
        let mut ids = vec![root];
        let total: u32 = seed.panes.iter().map(|pane| u32::from(pane.width)).sum();
        let mut remaining = total;
        for (index, entry) in seed.panes.iter().enumerate() {
            if index + 1 < seed.panes.len() {
                let Some(&target) = ids.last() else { break };
                let ratio = f32::from(entry.width) / remaining as f32;
                let Some(next) = layout.split_pane(target, Direction::Horizontal, ratio) else {
                    tracing::warn!(label = %entry.label, "could not split the dock for a seeded pane");
                    break;
                };
                ids.push(next);
                remaining -= u32::from(entry.width);
            }
        }
        for (id, entry) in ids.iter().zip(&seed.panes) {
            panes.insert(
                *id,
                DockPane::new(
                    DockPaneOrigin::Seeded {
                        label: entry.label.trim().to_owned(),
                        command: entry.command.clone(),
                    },
                    expand_cwd(entry.cwd.as_deref()),
                ),
            );
        }
        layout.focus_pane(root);
        if !self.rows_manual {
            self.rows = seed.rows;
        }
        self.state = DockState::Open(DockLayout {
            layout,
            panes,
            zoomed: false,
            stacks: Vec::new(),
        });
        self.spawn_all()
    }

    /// Spawn requests for every pane without a live process.
    fn spawn_all(&self) -> Vec<SpawnRequest> {
        let Some(dock) = self.open() else {
            return Vec::new();
        };
        // Spec 002: hidden stack members are started too.
        let mut ids = crate::workspace::stack::all_ids(&dock.layout, &dock.stacks);
        ids.sort_by_key(|id| id.raw());
        ids.into_iter()
            .filter_map(|id| {
                let pane = dock.panes.get(&id)?;
                (pane.terminal_id.is_none() && pane.respawn_at.is_none())
                    .then(|| self.spawn_request(id, pane))
            })
            .collect()
    }

    fn spawn_request(&self, pane_id: PaneId, pane: &DockPane) -> SpawnRequest {
        let (rows, cols) = self.size_of(pane_id);
        SpawnRequest {
            pane_id,
            cwd: pane.cwd.clone(),
            command: match &pane.origin {
                DockPaneOrigin::Seeded { command, .. } => Some(command.clone()),
                DockPaneOrigin::User => None,
            },
            rows,
            cols,
        }
    }

    /// Record that a pane's process started.
    pub(crate) fn started(&mut self, pane_id: PaneId, terminal_id: TerminalId, now: Instant) {
        if let Some(pane) = self
            .open_mut()
            .and_then(|dock| dock.panes.get_mut(&pane_id))
        {
            pane.terminal_id = Some(terminal_id);
            pane.started_at = Some(now);
            pane.respawn_at = None;
        }
    }

    /// A process failed to start: seeded panes retry with backoff, user panes close.
    pub(crate) fn spawn_failed(&mut self, pane_id: PaneId, now: Instant) -> DockExit {
        self.exited(pane_id, now)
    }

    /// Handle a dock pane's process exiting.
    pub(crate) fn exited(&mut self, pane_id: PaneId, now: Instant) -> DockExit {
        if self.retired.remove(&pane_id) {
            return DockExit::Retired;
        }
        let Some(dock) = self.open_mut() else {
            return DockExit::NotDock;
        };
        let Some(pane) = dock.panes.get_mut(&pane_id) else {
            return DockExit::NotDock;
        };
        pane.terminal_id = None;
        match pane.origin {
            DockPaneOrigin::Seeded { .. } => {
                let stayed_up = pane
                    .started_at
                    .is_some_and(|started| now.duration_since(started) >= RESPAWN_RESET_AFTER);
                if stayed_up {
                    pane.backoff = RESPAWN_INITIAL_DELAY;
                }
                let at = now + pane.backoff;
                pane.respawn_at = Some(at);
                pane.started_at = None;
                pane.backoff = (pane.backoff * 2).min(RESPAWN_MAX_DELAY);
                DockExit::Respawning(at)
            }
            DockPaneOrigin::User => {
                self.remove_pane(pane_id);
                DockExit::Closed
            }
        }
    }

    /// Earliest pending respawn.
    pub(crate) fn next_respawn(&self) -> Option<Instant> {
        self.open()?
            .panes
            .values()
            .filter_map(|pane| pane.respawn_at)
            .min()
    }

    /// Spawn requests for respawns that are due.
    pub(crate) fn due_respawns(&mut self, now: Instant) -> Vec<SpawnRequest> {
        let due = self
            .open()
            .map(|dock| {
                let mut ids = dock
                    .panes
                    .iter()
                    .filter(|(_, pane)| pane.respawn_at.is_some_and(|at| at <= now))
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();
                ids.sort_by_key(|id| id.raw());
                ids
            })
            .unwrap_or_default();
        let mut requests = Vec::new();
        for id in due {
            if let Some(pane) = self.open_mut().and_then(|dock| dock.panes.get_mut(&id)) {
                pane.respawn_at = None;
            }
            if let Some(pane) = self.open().and_then(|dock| dock.panes.get(&id)) {
                requests.push(self.spawn_request(id, pane));
            }
        }
        requests
    }

    /// Remove a pane from the layout, closing the dock when it was the last.
    /// Returns the terminal to shut down, if it was running.
    fn remove_pane(&mut self, pane_id: PaneId) -> Option<TerminalId> {
        let dock = self.open_mut()?;
        let pane = dock.panes.remove(&pane_id)?;
        if dock.panes.is_empty() {
            self.state = DockState::Closed;
        } else {
            // Spec 002: a stack member hands its slot on instead.
            if !crate::workspace::stack::unstack(&mut dock.layout, &mut dock.stacks, pane_id) {
                dock.layout.close_pane(pane_id);
            }
            if dock.layout.pane_count() == 1 {
                dock.zoomed = false;
            }
        }
        pane.terminal_id
    }

    /// Close a pane on purpose. Its later exit event is swallowed.
    pub(crate) fn close(&mut self, pane_id: PaneId) -> Option<TerminalId> {
        if !self.contains(pane_id) {
            return None;
        }
        let terminal = self.remove_pane(pane_id);
        if terminal.is_some() {
            self.retired.insert(pane_id);
        }
        terminal
    }

    /// Split `target`, adding a user pane that runs the default shell in
    /// `cwd`. Returns its spawn request.
    pub(crate) fn split(
        &mut self,
        target: PaneId,
        direction: Direction,
        cwd: PathBuf,
    ) -> Option<SpawnRequest> {
        let dock = self.open_mut()?;
        if !dock.panes.contains_key(&target) {
            return None;
        }
        let new_id = dock.layout.split_pane(target, direction, 0.5)?;
        dock.panes
            .insert(new_id, DockPane::new(DockPaneOrigin::User, cwd));
        dock.layout.focus_pane(new_id);
        dock.zoomed = false;
        let pane = self.open()?.panes.get(&new_id)?;
        Some(self.spawn_request(new_id, pane))
    }

    pub(crate) fn focus(&mut self, pane_id: PaneId) -> bool {
        let Some(dock) = self.open_mut() else {
            return false;
        };
        if !dock.panes.contains_key(&pane_id) {
            return false;
        }
        // Spec 002: a hidden stack member is shown first.
        crate::workspace::stack::reveal(&mut dock.layout, &mut dock.stacks, pane_id);
        dock.layout.focus_pane(pane_id);
        true
    }

    pub(crate) fn toggle_zoom(&mut self) -> bool {
        let Some(dock) = self.open_mut() else {
            return false;
        };
        if dock.layout.pane_count() < 2 && !dock.zoomed {
            return false;
        }
        dock.zoomed = !dock.zoomed;
        true
    }

    pub(crate) fn set_ratio(&mut self, path: &[bool], ratio: f32) -> bool {
        self.open_mut()
            .is_some_and(|dock| dock.layout.set_ratio_at(path, ratio))
    }

    pub(crate) fn resize_focused(&mut self, nav: NavDirection, delta: f32) -> bool {
        let area = self.area();
        let Some(dock) = self.open_mut() else {
            return false;
        };
        dock.layout.resize_focused(nav, delta, area);
        true
    }

    /// Set the shared height. Returns whether it changed.
    pub(crate) fn set_rows(&mut self, rows: u16) -> bool {
        let rows = rows.clamp(1, MAX_DOCK_ROWS);
        if rows == self.rows {
            return false;
        }
        self.rows = rows;
        self.rows_manual = true;
        true
    }

    /// Put the height back to the config seed's.
    pub(crate) fn reset_rows(&mut self) -> bool {
        let changed = self.rows != self.seed.rows;
        self.rows = self.seed.rows;
        self.rows_manual = false;
        changed
    }

    /// Discard the runtime dock and rebuild it from the seed. Returns the
    /// terminals to shut down and the processes to start.
    pub(crate) fn reset(&mut self) -> (Vec<TerminalId>, Vec<SpawnRequest>) {
        let stopping = self.take_all_terminals();
        self.rows_manual = false;
        self.rows = self.seed.rows;
        let spawns = self.build_from_seed();
        (stopping, spawns)
    }

    /// Detach every live terminal, marking their panes retired.
    fn take_all_terminals(&mut self) -> Vec<TerminalId> {
        let mut stopping = Vec::new();
        let mut retired = Vec::new();
        if let Some(dock) = self.open_mut() {
            for (id, pane) in dock.panes.iter_mut() {
                if let Some(terminal) = pane.terminal_id.take() {
                    stopping.push(terminal);
                    retired.push(*id);
                }
            }
        }
        self.retired.extend(retired);
        self.state = DockState::Unbuilt;
        stopping
    }

    /// Replace the seed after a config reload. A running or closed dock is
    /// left alone; an unbuilt one is built when the new seed is valid.
    pub(crate) fn apply_seed(&mut self, seed: DockConfig) -> Vec<SpawnRequest> {
        self.seed = seed;
        if matches!(self.state, DockState::Unbuilt) {
            self.build_from_seed()
        } else {
            Vec::new()
        }
    }

    /// Record the dock width reported by a client. Returns whether it changed.
    pub(crate) fn set_width(&mut self, width: u16) -> bool {
        let width = width.clamp(1, MAX_DOCK_WIDTH);
        if width == self.width {
            return false;
        }
        self.width = width;
        true
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

impl Dock {
    /// Saved form. `None` while unbuilt, so a later seed can still apply.
    pub(crate) fn to_snapshot(&self) -> Option<crate::persist::DockSnapshot> {
        let mut snapshot = crate::persist::DockSnapshot {
            rows: self.rows,
            rows_manual: self.rows_manual,
            layout: None,
            panes: HashMap::new(),
            focused: None,
            zoomed: false,
            stacks: Vec::new(),
        };
        match &self.state {
            DockState::Unbuilt => return None,
            DockState::Closed => {}
            DockState::Open(dock) => {
                snapshot.layout = Some(crate::persist::capture_layout(dock.layout.root()));
                snapshot.focused = Some(dock.layout.focused().raw());
                snapshot.zoomed = dock.zoomed;
                snapshot.stacks = dock
                    .stacks
                    .iter()
                    .map(|stack| crate::persist::StackSnapshot {
                        panes: stack.panes.iter().map(|id| id.raw()).collect(),
                        active: stack.active,
                    })
                    .collect();
                snapshot.panes = dock
                    .panes
                    .iter()
                    .map(|(id, pane)| {
                        let (label, command) = match &pane.origin {
                            DockPaneOrigin::Seeded { label, command } => {
                                (Some(label.clone()), Some(command.clone()))
                            }
                            DockPaneOrigin::User => (None, None),
                        };
                        (
                            id.raw(),
                            crate::persist::DockPaneSnapshot {
                                cwd: pane.cwd.clone(),
                                label,
                                command,
                            },
                        )
                    })
                    .collect();
            }
        }
        Some(snapshot)
    }

    /// Restore a saved dock. Panes get fresh ids and no processes yet; call
    /// `spawn_pending` to start them.
    pub(crate) fn restore(&mut self, snapshot: &crate::persist::DockSnapshot) {
        self.rows = snapshot.rows.clamp(1, MAX_DOCK_ROWS);
        self.rows_manual = snapshot.rows_manual;
        let Some(saved_layout) = &snapshot.layout else {
            self.state = DockState::Closed;
            return;
        };
        let (root, mut id_map) = crate::persist::restore_layout(saved_layout);
        // Spec 002: hidden stack members get fresh ids alongside the tree.
        let tree_ids: std::collections::HashSet<u32> = id_map.keys().copied().collect();
        let stack_plans = restore_stack_plans(snapshot, &tree_ids);
        for (members, visible) in &stack_plans {
            for old in members {
                if old != visible {
                    id_map.insert(*old, PaneId::alloc());
                }
            }
        }
        let stacks = stack_plans
            .iter()
            .filter_map(|(members, visible)| {
                let panes = members
                    .iter()
                    .filter_map(|old| id_map.get(old).copied())
                    .collect::<Vec<_>>();
                let active = members.iter().position(|old| old == visible)?;
                (panes.len() >= 2).then_some(crate::workspace::PaneStack { panes, active })
            })
            .collect::<Vec<_>>();
        let panes = id_map
            .iter()
            .map(|(old, new)| {
                let saved = snapshot.panes.get(old);
                let origin =
                    match saved.and_then(|pane| pane.label.clone().zip(pane.command.clone())) {
                        Some((label, command)) => DockPaneOrigin::Seeded { label, command },
                        None => DockPaneOrigin::User,
                    };
                let cwd = saved.map_or_else(|| expand_cwd(None), |pane| pane.cwd.clone());
                (*new, DockPane::new(origin, cwd))
            })
            .collect::<HashMap<_, _>>();
        let focus = snapshot
            .focused
            .filter(|old| tree_ids.contains(old))
            .and_then(|old| id_map.get(&old).copied())
            .or_else(|| {
                let mut ids = tree_ids
                    .iter()
                    .filter_map(|old| id_map.get(old).copied())
                    .collect::<Vec<_>>();
                ids.sort_by_key(|id| id.raw());
                ids.first().copied()
            });
        let Some(focus) = focus else {
            self.state = DockState::Closed;
            return;
        };
        let layout = TileLayout::from_saved(root, focus);
        self.state = DockState::Open(DockLayout {
            zoomed: snapshot.zoomed && layout.pane_count() > 1,
            layout,
            panes,
            stacks,
        });
    }

    /// Spawn requests for every pane without a process and no pending respawn.
    pub(crate) fn spawn_pending(&self) -> Vec<SpawnRequest> {
        self.spawn_all()
    }
}

// ---------------------------------------------------------------------------
// Server glue
// ---------------------------------------------------------------------------

impl crate::app::App {
    /// Build or restore the dock at startup. A saved dock wins over the seed.
    pub(crate) fn init_dock(
        &mut self,
        seed: &DockConfig,
        saved: Option<&crate::persist::DockSnapshot>,
    ) {
        self.state.dock.seed = seed.clone();
        let spawns = match saved {
            Some(snapshot) => {
                self.state.dock.restore(snapshot);
                self.state.dock.spawn_pending()
            }
            None => self.state.dock.build_from_seed(),
        };
        self.spawn_dock_processes(spawns);
    }

    fn spawn_dock_processes(&mut self, spawns: Vec<SpawnRequest>) {
        if !spawns.is_empty() {
            self.state.dock.needs_redraw = true;
        }
        let now = Instant::now();
        for spawn in spawns {
            let terminal_id = TerminalId::alloc();
            let launch_env =
                crate::pane::PaneLaunchEnv::from_extra(Vec::new()).without_pane_identity();
            let result = match &spawn.command {
                Some(command) => crate::terminal::TerminalRuntime::spawn_shell_command(
                    spawn.pane_id,
                    spawn.rows,
                    spawn.cols,
                    spawn.cwd.clone(),
                    command,
                    &launch_env,
                    crate::pane::AgentDetection::Disabled,
                    self.state.pane_scrollback_limit_bytes,
                    self.state.host_terminal_theme,
                    self.state.host_terminal_appearance,
                    self.event_tx.clone(),
                    self.render_notify.clone(),
                    self.render_dirty.clone(),
                ),
                None => crate::terminal::TerminalRuntime::spawn(
                    spawn.pane_id,
                    spawn.rows,
                    spawn.cols,
                    spawn.cwd.clone(),
                    self.state.pane_scrollback_limit_bytes,
                    self.state.host_terminal_theme,
                    self.state.host_terminal_appearance,
                    crate::pane::PaneShellConfig::new(
                        &self.state.default_shell,
                        self.state.shell_mode,
                    ),
                    &launch_env,
                    self.event_tx.clone(),
                    self.render_notify.clone(),
                    self.render_dirty.clone(),
                ),
            };
            match result {
                Ok(runtime) => {
                    tracing::info!(
                        pane = spawn.pane_id.raw(),
                        command = spawn.command.as_deref().unwrap_or("<shell>"),
                        "dock: started pane process"
                    );
                    self.terminal_runtimes.insert(terminal_id.clone(), runtime);
                    self.state.terminals.insert(
                        terminal_id.clone(),
                        crate::terminal::TerminalState::new(terminal_id.clone(), spawn.cwd),
                    );
                    self.state.dock.started(spawn.pane_id, terminal_id, now);
                }
                Err(err) => {
                    tracing::warn!(
                        pane = spawn.pane_id.raw(),
                        %err,
                        "dock: failed to start pane process"
                    );
                    self.state.dock.spawn_failed(spawn.pane_id, now);
                }
            }
        }
        self.dock_changed();
    }

    fn stop_dock_terminal(&mut self, terminal_id: TerminalId) {
        self.state.terminals.remove(&terminal_id);
        self.shutdown_terminal_runtime(terminal_id);
    }

    fn dock_changed(&mut self) {
        self.state.dock.needs_redraw = true;
        self.resize_dock_runtimes();
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
    }

    /// Changes that should be saved with the session.
    fn dock_layout_changed(&mut self) {
        self.state.mark_session_dirty();
        self.dock_changed();
    }

    fn resize_dock_runtimes(&mut self) {
        let Some(dock) = self.state.dock.open() else {
            return;
        };
        let sizes = self.state.dock.pane_sizes();
        for (pane_id, pane) in &dock.panes {
            let (Some(terminal_id), Some((rows, cols))) = (&pane.terminal_id, sizes.get(pane_id))
            else {
                continue;
            };
            if let Some(runtime) = self.terminal_runtimes.get(terminal_id) {
                runtime.resize(*rows, *cols, 0, 0);
            }
        }
    }

    /// Handle a pane exit if it belongs to the dock. Returns false otherwise.
    pub(crate) fn handle_dock_pane_exit(&mut self, pane_id: PaneId) -> bool {
        let terminal = self
            .state
            .dock
            .open()
            .and_then(|dock| dock.panes.get(&pane_id))
            .and_then(|pane| pane.terminal_id.clone());
        match self.state.dock.exited(pane_id, Instant::now()) {
            DockExit::NotDock => false,
            DockExit::Retired => true,
            DockExit::Respawning(at) => {
                if let Some(terminal) = terminal {
                    self.stop_dock_terminal(terminal);
                }
                tracing::info!(
                    pane = pane_id.raw(),
                    in_ms = at.saturating_duration_since(Instant::now()).as_millis() as u64,
                    "dock: pane exited, respawning"
                );
                self.dock_changed();
                true
            }
            DockExit::Closed => {
                if let Some(terminal) = terminal {
                    self.stop_dock_terminal(terminal);
                }
                self.dock_layout_changed();
                true
            }
        }
    }

    /// Next time the server loop must wake for the dock: a due respawn, or
    /// the next countdown tick while one is pending.
    pub(crate) fn next_dock_deadline(&self) -> Option<Instant> {
        let respawn = self.state.dock.next_respawn()?;
        Some(respawn.min(Instant::now() + Duration::from_secs(1)))
    }

    /// Respawn seeded programs whose backoff has run out. Returns whether the
    /// dock needs redrawing, which includes a running restart countdown.
    pub(crate) fn run_due_dock_respawns(&mut self, now: Instant) -> bool {
        let spawns = self.state.dock.due_respawns(now);
        if spawns.is_empty() {
            let counting_down = self.dock_waiting();
            self.state.dock.needs_redraw |= counting_down;
            return counting_down;
        }
        self.spawn_dock_processes(spawns);
        true
    }

    /// Saved dock, with each pane's live working directory.
    pub(crate) fn dock_snapshot(&self) -> Option<crate::persist::DockSnapshot> {
        let mut snapshot = self.state.dock.to_snapshot()?;
        if let Some(dock) = self.state.dock.open() {
            for (id, pane) in &dock.panes {
                let live = pane
                    .terminal_id
                    .as_ref()
                    .and_then(|terminal| self.terminal_runtimes.get(terminal))
                    .and_then(|runtime| runtime.cwd_for_persistence());
                if let (Some(cwd), Some(saved)) = (live, snapshot.panes.get_mut(&id.raw())) {
                    saved.cwd = cwd;
                }
            }
        }
        Some(snapshot)
    }

    /// Config reload: a running or closed dock is left alone.
    pub(crate) fn apply_dock_seed(&mut self, seed: &DockConfig) {
        let spawns = self.state.dock.apply_seed(seed.clone());
        if !spawns.is_empty() {
            self.spawn_dock_processes(spawns);
            self.state.mark_session_dirty();
        }
    }

    pub(crate) fn reset_dock(&mut self) {
        let (stopping, spawns) = self.state.dock.reset();
        for terminal in stopping {
            self.stop_dock_terminal(terminal);
        }
        self.spawn_dock_processes(spawns);
        self.dock_layout_changed();
    }

    pub(crate) fn dock_set_width(&mut self, width: u16) {
        if self.state.dock.set_width(width) {
            self.dock_changed();
        }
    }

    pub(crate) fn dock_set_rows(&mut self, rows: u16) {
        if self.state.dock.set_rows(rows) {
            self.dock_layout_changed();
        }
    }

    pub(crate) fn dock_reset_rows(&mut self) {
        if self.state.dock.reset_rows() {
            self.dock_layout_changed();
        }
    }

    /// Split a dock pane; the new pane starts the default shell in the
    /// source pane's working directory.
    pub(crate) fn dock_split(&mut self, target: PaneId, direction: Direction) -> bool {
        let Some(cwd) = self
            .state
            .dock
            .open()
            .and_then(|dock| dock.panes.get(&target))
            .map(|pane| self.dock_pane_cwd(pane))
        else {
            return false;
        };
        let Some(spawn) = self.state.dock.split(target, direction, cwd) else {
            return false;
        };
        self.spawn_dock_processes(vec![spawn]);
        self.state.mark_session_dirty();
        true
    }

    /// Spec 002: a new shell as a member of `target`'s stack, in its cwd.
    pub(crate) fn dock_stack_new(&mut self, target: PaneId) -> bool {
        let Some(cwd) = self
            .state
            .dock
            .open()
            .and_then(|dock| dock.panes.get(&target))
            .map(|pane| self.dock_pane_cwd(pane))
        else {
            return false;
        };
        let Some(spawn) = self.state.dock.stack_new(target, cwd) else {
            return false;
        };
        self.spawn_dock_processes(vec![spawn]);
        self.dock_layout_changed();
        true
    }

    /// The live working directory of a dock pane, falling back to the one it
    /// started in.
    fn dock_pane_cwd(&self, pane: &DockPane) -> PathBuf {
        pane.terminal_id
            .as_ref()
            .and_then(|terminal| self.terminal_runtimes.get(terminal))
            .and_then(|runtime| runtime.cwd())
            .unwrap_or_else(|| pane.cwd.clone())
    }

    pub(crate) fn dock_close(&mut self, pane_id: PaneId) -> bool {
        if !self.state.dock.contains(pane_id) {
            return false;
        }
        if let Some(terminal) = self.state.dock.close(pane_id) {
            self.stop_dock_terminal(terminal);
        }
        self.dock_layout_changed();
        true
    }

    pub(crate) fn dock_focus(&mut self, pane_id: PaneId) -> bool {
        let changed = self.state.dock.focus(pane_id);
        if changed {
            self.state.mark_session_dirty();
            self.render_dirty.request_generic();
        }
        changed
    }

    pub(crate) fn dock_toggle_zoom(&mut self) -> bool {
        let changed = self.state.dock.toggle_zoom();
        if changed {
            self.dock_layout_changed();
        }
        changed
    }

    pub(crate) fn dock_set_ratio(&mut self, path: &[bool], ratio: f32) -> bool {
        let changed = self.state.dock.set_ratio(path, ratio);
        if changed {
            self.dock_layout_changed();
        }
        changed
    }

    pub(crate) fn dock_resize_focused(&mut self, nav: NavDirection, delta: f32) -> bool {
        let changed = self.state.dock.resize_focused(nav, delta);
        if changed {
            self.dock_layout_changed();
        }
        changed
    }
}

// ---------------------------------------------------------------------------
// Client controls
// ---------------------------------------------------------------------------

/// Resize-mode step for dividers, as a share of the split.
const DOCK_RESIZE_STEP: f32 = 0.05;

fn nav(direction: crate::protocol::endpoint::DockDirection) -> Option<NavDirection> {
    use crate::protocol::endpoint::DockDirection;
    match direction {
        DockDirection::Left => Some(NavDirection::Left),
        DockDirection::Right => Some(NavDirection::Right),
        DockDirection::Up => Some(NavDirection::Up),
        DockDirection::Down => Some(NavDirection::Down),
        DockDirection::Unknown => None,
    }
}

impl crate::app::App {
    /// Apply one client dock operation other than input. Returns whether the
    /// dock needs redrawing. Unknown pane ids are ignored: a pane can close
    /// between the client drawing it and the operation arriving.
    pub(crate) fn apply_dock_control(
        &mut self,
        control: crate::protocol::endpoint::DockControl,
    ) -> bool {
        use crate::protocol::endpoint::{DockControl, DockDirection};
        let pane = |id: &str| parse_dock_pane_id(id).filter(|id| self.state.dock.contains(*id));
        match control {
            DockControl::Width { cols } => {
                self.dock_set_width(cols);
                true
            }
            DockControl::Input { .. } | DockControl::Unknown => false,
            DockControl::Split { pane_id, right } => pane(&pane_id).is_some_and(|id| {
                self.dock_split(
                    id,
                    if right {
                        Direction::Horizontal
                    } else {
                        Direction::Vertical
                    },
                )
            }),
            DockControl::Close { pane_id } => pane(&pane_id).is_some_and(|id| self.dock_close(id)),
            DockControl::Focus { pane_id } => pane(&pane_id).is_some_and(|id| self.dock_focus(id)),
            DockControl::FocusDirection { direction } => {
                let changed = nav(direction)
                    .is_some_and(|direction| self.state.dock.focus_direction(direction));
                if changed {
                    self.state.mark_session_dirty();
                }
                changed
            }
            DockControl::ToggleZoom => self.dock_toggle_zoom(),
            DockControl::StackNew { pane_id } => {
                pane(&pane_id).is_some_and(|id| self.dock_stack_new(id))
            }
            DockControl::StackCycle { pane_id, forward } => pane(&pane_id).is_some_and(|id| {
                let changed = self.state.dock.stack_cycle(id, forward);
                if changed {
                    self.dock_layout_changed();
                }
                changed
            }),
            DockControl::Ratio { path, ratio } => self.dock_set_ratio(&path, ratio),
            DockControl::Resize { direction } => match direction {
                DockDirection::Up | DockDirection::Down
                    if self.state.dock.focused_touches_top() =>
                {
                    let rows = self.state.dock.rows;
                    let next = if direction == DockDirection::Up {
                        rows.saturating_add(1)
                    } else {
                        rows.saturating_sub(1).max(1)
                    };
                    self.dock_set_rows(next);
                    true
                }
                _ => nav(direction)
                    .is_some_and(|direction| self.dock_resize_focused(direction, DOCK_RESIZE_STEP)),
            },
            DockControl::Rows { rows } => {
                self.dock_set_rows(rows);
                true
            }
            DockControl::ResetRows => {
                self.dock_reset_rows();
                true
            }
            DockControl::Reset => {
                self.reset_dock();
                true
            }
        }
    }

    /// The live runtime of a dock pane, for routing client input.
    pub(crate) fn dock_runtime(&self, pane_id: &str) -> Option<&crate::terminal::TerminalRuntime> {
        let id = parse_dock_pane_id(pane_id)?;
        let terminal = self
            .state
            .dock
            .open()?
            .panes
            .get(&id)?
            .terminal_id
            .as_ref()?;
        self.terminal_runtimes.get(terminal)
    }

    /// Whether a seeded pane is waiting to restart, so its countdown needs
    /// redrawing about once a second.
    pub(crate) fn dock_waiting(&self) -> bool {
        self.state.dock.next_respawn().is_some()
    }
}

#[cfg(test)]
impl crate::app::App {
    /// Build a seeded dock whose panes use the given runtimes instead of
    /// spawned processes. Returns the pane ids, left to right.
    pub(crate) fn install_test_dock(
        &mut self,
        panes: Vec<(&str, crate::terminal::TerminalRuntime)>,
    ) -> Vec<PaneId> {
        self.state.dock.seed = DockConfig {
            rows: 3,
            panes: panes
                .iter()
                .map(|(label, _)| crate::config::DockPaneConfig {
                    label: (*label).into(),
                    command: "test".into(),
                    cwd: Some(PathBuf::from("/tmp")),
                    width: 1,
                })
                .collect(),
        };
        let spawns = self.state.dock.build_from_seed();
        let now = Instant::now();
        let mut ids = Vec::new();
        for (spawn, (_, runtime)) in spawns.iter().zip(panes) {
            let terminal = TerminalId::alloc();
            self.terminal_runtimes.insert(terminal.clone(), runtime);
            self.state.terminals.insert(
                terminal.clone(),
                crate::terminal::TerminalState::new(terminal.clone(), PathBuf::from("/tmp")),
            );
            self.state.dock.started(spawn.pane_id, terminal, now);
            ids.push(spawn.pane_id);
        }
        ids
    }
}

// ---------------------------------------------------------------------------
// Pane stacks in the dock (spec 002, ushineko/specs/002-nested-tabs.md)
// ---------------------------------------------------------------------------

/// Split a dock pane's area into its strip row and the program's area. Only a
/// stack's visible member has a strip, and only with two or more rows.
fn stack_strip(dock: &DockLayout, pane_id: PaneId, inner: Rect) -> (Option<Rect>, Rect) {
    let stacked = crate::workspace::stack::stack_of(&dock.stacks, pane_id)
        .is_some_and(|stack| stack.visible() == Some(pane_id));
    if !stacked || inner.height < 2 {
        return (None, inner);
    }
    (
        Some(Rect::new(inner.x, inner.y, inner.width, 1)),
        Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1),
    )
}

/// Saved dock stacks that can be rebuilt: exactly one member in the saved
/// tree (the visible one), hidden members with a saved pane entry, each pane in
/// one stack, two or more members. Returns (members, visible) by saved id.
fn restore_stack_plans(
    snapshot: &crate::persist::DockSnapshot,
    tree_ids: &HashSet<u32>,
) -> Vec<(Vec<u32>, u32)> {
    let mut claimed = HashSet::new();
    let mut plans = Vec::new();
    for stack in &snapshot.stacks {
        let mut members = Vec::new();
        let mut visible = None;
        for old in &stack.panes {
            if claimed.contains(old) || members.contains(old) {
                continue;
            }
            if tree_ids.contains(old) {
                if visible.is_none() {
                    visible = Some(*old);
                    members.push(*old);
                }
            } else if snapshot.panes.contains_key(old) {
                members.push(*old);
            }
        }
        match visible {
            Some(visible) if members.len() >= 2 => {
                claimed.extend(members.iter().copied());
                plans.push((members, visible));
            }
            _ => tracing::warn!(
                members = stack.panes.len(),
                "dock: dropped a saved pane stack without a slot or members"
            ),
        }
    }
    plans
}

impl Dock {
    /// Whether a dock pane is a hidden stack member.
    pub(crate) fn is_hidden(&self, pane_id: PaneId) -> bool {
        self.open()
            .is_some_and(|dock| crate::workspace::stack::is_hidden(&dock.stacks, pane_id))
    }

    /// Add a user pane running the default shell in `cwd` as a new member of
    /// `target`'s stack, shown and focused. Returns its spawn request.
    pub(crate) fn stack_new(&mut self, target: PaneId, cwd: PathBuf) -> Option<SpawnRequest> {
        let dock = self.open_mut()?;
        if !dock.panes.contains_key(&target) {
            return None;
        }
        let new_id = PaneId::alloc();
        if !crate::workspace::stack::push_member(&mut dock.layout, &mut dock.stacks, target, new_id)
        {
            return None;
        }
        dock.panes
            .insert(new_id, DockPane::new(DockPaneOrigin::User, cwd));
        dock.zoomed = false;
        let pane = self.open()?.panes.get(&new_id)?;
        Some(self.spawn_request(new_id, pane))
    }

    /// Show the next (or previous) member of `pane_id`'s stack, focused.
    pub(crate) fn stack_cycle(&mut self, pane_id: PaneId, forward: bool) -> bool {
        let Some(target) = self
            .open()
            .and_then(|dock| crate::workspace::stack::neighbour(&dock.stacks, pane_id, forward))
        else {
            return false;
        };
        self.focus(target)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::DockPaneConfig;

    pub(crate) fn seed(entries: &[(&str, &str, u16)]) -> DockConfig {
        DockConfig {
            rows: 3,
            panes: entries
                .iter()
                .map(|(label, command, width)| DockPaneConfig {
                    label: (*label).into(),
                    command: (*command).into(),
                    cwd: Some(PathBuf::from("/tmp")),
                    width: *width,
                })
                .collect(),
        }
    }

    pub(crate) fn seeded_dock(entries: &[(&str, &str, u16)]) -> (Dock, Vec<SpawnRequest>) {
        let mut dock = Dock {
            seed: seed(entries),
            width: 100,
            ..Dock::default()
        };
        let spawns = dock.build_from_seed();
        (dock, spawns)
    }

    pub(crate) fn start_all(dock: &mut Dock, spawns: &[SpawnRequest], now: Instant) {
        for spawn in spawns {
            dock.started(spawn.pane_id, TerminalId::alloc(), now);
        }
    }

    fn widths_left_to_right(dock: &Dock) -> Vec<(String, u16)> {
        let layout = dock.open().unwrap();
        let mut infos = layout.layout.panes(dock.area());
        infos.sort_by_key(|info| info.rect.x);
        infos
            .into_iter()
            .map(|info| {
                (
                    layout.panes[&info.id].label().unwrap_or("user").to_owned(),
                    info.rect.width,
                )
            })
            .collect()
    }

    #[test]
    fn seed_builds_one_row_weighted_left_to_right() {
        let (dock, spawns) = seeded_dock(&[("usage", "hayami", 3), ("net", "htop", 1)]);
        assert_eq!(dock.rows, 3);
        let widths = widths_left_to_right(&dock);
        assert_eq!(widths[0].0, "usage");
        assert_eq!(widths[1].0, "net");
        assert!(
            (widths[0].1 as i32 - 75).abs() <= 1,
            "3:1 weights give about 75 columns: {widths:?}"
        );
        assert_eq!(spawns.len(), 2);
        assert!(spawns.iter().all(|spawn| spawn.rows == 3));
        assert_eq!(
            spawns
                .iter()
                .map(|spawn| spawn.command.clone().unwrap())
                .collect::<Vec<_>>(),
            vec!["hayami".to_owned(), "htop".to_owned()],
            "spawned in layout order"
        );
    }

    #[test]
    fn three_equal_weights_split_evenly() {
        let (dock, _) = seeded_dock(&[("a", "x", 1), ("b", "y", 1), ("c", "z", 1)]);
        let widths = widths_left_to_right(&dock)
            .into_iter()
            .map(|(_, width)| width as i32)
            .collect::<Vec<_>>();
        assert!(
            widths.iter().all(|width| (width - 33).abs() <= 1),
            "{widths:?}"
        );
    }

    #[test]
    fn no_or_invalid_seed_builds_nothing() {
        let mut dock = Dock::default();
        assert!(dock.build_from_seed().is_empty());
        assert!(matches!(dock.state, DockState::Unbuilt));

        let mut dock = Dock {
            seed: seed(&[("a", "", 1)]),
            ..Dock::default()
        };
        assert!(dock.build_from_seed().is_empty());
        assert!(matches!(dock.state, DockState::Unbuilt));
    }

    #[test]
    fn seeded_exit_respawns_with_doubling_backoff_capped_at_thirty_seconds() {
        let t0 = Instant::now();
        let (mut dock, spawns) = seeded_dock(&[("usage", "hayami", 1)]);
        let pane = spawns[0].pane_id;
        start_all(&mut dock, &spawns, t0);

        let mut now = t0 + Duration::from_secs(2);
        let mut delays = Vec::new();
        for _ in 0..7 {
            let DockExit::Respawning(at) = dock.exited(pane, now) else {
                panic!("a seeded pane respawns");
            };
            delays.push(at.duration_since(now).as_secs());
            assert_eq!(dock.next_respawn(), Some(at));
            assert!(dock.due_respawns(at - Duration::from_millis(1)).is_empty());
            let due = dock.due_respawns(at);
            assert_eq!(due.len(), 1);
            assert_eq!(due[0].command.as_deref(), Some("hayami"));
            dock.started(pane, TerminalId::alloc(), at);
            now = at + Duration::from_secs(1);
        }
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn backoff_resets_after_a_long_run() {
        let t0 = Instant::now();
        let (mut dock, spawns) = seeded_dock(&[("usage", "hayami", 1)]);
        let pane = spawns[0].pane_id;
        start_all(&mut dock, &spawns, t0);
        for _ in 0..4 {
            let DockExit::Respawning(at) = dock.exited(pane, t0) else {
                panic!()
            };
            dock.due_respawns(at);
            dock.started(pane, TerminalId::alloc(), t0);
        }
        let later = t0 + RESPAWN_RESET_AFTER + Duration::from_secs(1);
        let DockExit::Respawning(at) = dock.exited(pane, later) else {
            panic!()
        };
        assert_eq!(at.duration_since(later), RESPAWN_INITIAL_DELAY);
    }

    #[test]
    fn user_pane_closes_on_exit_and_last_close_leaves_a_closed_dock() {
        let t0 = Instant::now();
        let (mut dock, spawns) = seeded_dock(&[("usage", "hayami", 1)]);
        let seeded = spawns[0].pane_id;
        start_all(&mut dock, &spawns, t0);

        let split = dock
            .split(seeded, Direction::Vertical, PathBuf::from("/tmp"))
            .expect("split a dock pane");
        assert_eq!(split.command, None, "a split runs the default shell");
        dock.started(split.pane_id, TerminalId::alloc(), t0);
        assert_eq!(dock.open().unwrap().panes.len(), 2);

        assert_eq!(dock.exited(split.pane_id, t0), DockExit::Closed);
        assert_eq!(dock.open().unwrap().panes.len(), 1);

        let terminal = dock.close(seeded);
        assert!(terminal.is_some(), "the seeded process is shut down");
        assert!(matches!(dock.state, DockState::Closed));
        assert_eq!(
            dock.exited(seeded, t0),
            DockExit::Retired,
            "the exit of a closed pane does not respawn it"
        );
    }

    #[test]
    fn reset_rebuilds_from_the_seed_and_retires_old_processes() {
        let t0 = Instant::now();
        let (mut dock, spawns) = seeded_dock(&[("usage", "hayami", 1)]);
        start_all(&mut dock, &spawns, t0);
        let old = spawns[0].pane_id;
        dock.split(old, Direction::Horizontal, PathBuf::from("/tmp"));
        dock.set_rows(8);

        let (stopping, respawns) = dock.reset();
        assert_eq!(stopping.len(), 1, "only live processes are stopped");
        assert_eq!(respawns.len(), 1);
        assert_eq!(dock.rows, 3, "height returns to the seed's");
        assert_eq!(dock.open().unwrap().panes.len(), 1);
        assert_eq!(dock.exited(old, t0), DockExit::Retired);
    }

    #[test]
    fn reload_leaves_a_built_dock_alone_but_builds_an_unbuilt_one() {
        let (mut dock, _) = seeded_dock(&[("usage", "hayami", 1)]);
        let before = dock.open().unwrap().layout.pane_ids();
        let spawns = dock.apply_seed(seed(&[("a", "x", 1), ("b", "y", 1)]));
        assert!(spawns.is_empty());
        assert_eq!(dock.open().unwrap().layout.pane_ids(), before);

        let mut unbuilt = Dock::default();
        let spawns = unbuilt.apply_seed(seed(&[("a", "x", 1)]));
        assert_eq!(spawns.len(), 1);
        assert!(unbuilt.open().is_some());
    }

    #[test]
    fn manual_rows_survive_a_reseed_until_reset() {
        let mut dock = Dock {
            seed: seed(&[("a", "x", 1)]),
            ..Dock::default()
        };
        dock.set_rows(9);
        dock.build_from_seed();
        assert_eq!(dock.rows, 9);
        assert!(dock.reset_rows());
        assert_eq!(dock.rows, 3);
    }

    #[test]
    fn directional_focus_moves_between_dock_panes_and_stops_at_the_edges() {
        let (mut dock, spawns) = seeded_dock(&[("a", "x", 1), ("b", "y", 1)]);
        let (left, right) = (spawns[0].pane_id, spawns[1].pane_id);
        assert_eq!(dock.open().unwrap().layout.focused(), left);
        assert!(
            !dock.focus_direction(NavDirection::Left),
            "nothing left of the first pane"
        );
        assert!(
            !dock.focus_direction(NavDirection::Up),
            "the dock's top edge stops focus"
        );
        assert!(dock.focus_direction(NavDirection::Right));
        assert_eq!(dock.open().unwrap().layout.focused(), right);
        assert!(!dock.focus_direction(NavDirection::Right));
    }

    #[test]
    fn resize_mode_left_and_right_move_the_divider() {
        let (mut dock, _) = seeded_dock(&[("a", "x", 1), ("b", "y", 1)]);
        let width = |dock: &Dock| {
            dock.geometry()
                .iter()
                .find(|pane| pane.focused)
                .map(|pane| pane.rect.width)
                .unwrap()
        };
        let before = width(&dock);
        assert!(dock.resize_focused(NavDirection::Right, 0.1));
        assert!(width(&dock) > before, "the focused pane grows to the right");
        assert!(dock.resize_focused(NavDirection::Left, 0.2));
        assert!(width(&dock) < before);
    }

    #[test]
    fn client_supplied_sizes_are_clamped() {
        let mut dock = Dock::default();
        dock.set_rows(u16::MAX);
        dock.set_width(u16::MAX);
        assert_eq!((dock.rows, dock.width), (MAX_DOCK_ROWS, MAX_DOCK_WIDTH));
        dock.set_rows(0);
        dock.set_width(0);
        assert_eq!((dock.rows, dock.width), (1, 1));
    }

    #[test]
    fn pane_sizes_follow_width_height_and_zoom() {
        let (mut dock, spawns) = seeded_dock(&[("a", "x", 1), ("b", "y", 1)]);
        dock.set_width(60);
        dock.set_rows(5);
        let sizes = dock.pane_sizes();
        assert!(sizes.values().all(|(rows, cols)| *rows == 5 && *cols <= 30));
        dock.focus(spawns[1].pane_id);
        assert!(dock.toggle_zoom());
        assert!(dock.pane_sizes().values().all(|size| *size == (5, 60)));
    }
}

/// `App`-level tests with real processes. Unix only: they run `sleep`.
#[cfg(all(test, unix))]
mod app_tests {
    use super::tests::seed;
    use super::*;
    use crate::app::App;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        // Split panes start this and stay up long enough to be observed.
        app.state.default_shell = "/bin/sh".into();
        app
    }

    /// Stop every dock process the test started.
    fn shut_down(app: &mut App) {
        let ids = app
            .state
            .dock
            .open()
            .map(|dock| dock.panes.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        for id in ids {
            app.dock_close(id);
        }
    }

    fn live_terminals(app: &App) -> Vec<TerminalId> {
        let mut ids = app
            .state
            .dock
            .open()
            .map(|dock| {
                dock.panes
                    .values()
                    .filter_map(|pane| pane.terminal_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ids
    }

    fn seeded_pane(app: &App, label: &str) -> PaneId {
        let dock = app.state.dock.open().expect("dock is open");
        *dock
            .panes
            .iter()
            .find(|(_, pane)| pane.label() == Some(label))
            .expect("seeded pane")
            .0
    }

    fn pane_list_len(app: &mut App) -> usize {
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "dock-pane-list".into(),
            method: crate::api::schema::Method::PaneList(crate::api::schema::PaneListParams {
                workspace_id: None,
            }),
        });
        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        let crate::api::schema::ResponseResult::PaneList { panes } = parsed.result else {
            panic!("expected a pane list: {response}");
        };
        panes.len()
    }

    #[tokio::test]
    async fn seed_starts_one_process_per_entry_outside_every_workspace() {
        let mut app = test_app();
        let panes_before = pane_list_len(&mut app);
        let workspaces_before = app.state.workspaces.len();

        app.init_dock(
            &seed(&[("usage", "sleep 5", 3), ("net", "sleep 5", 1)]),
            None,
        );

        let terminals = live_terminals(&app);
        assert_eq!(terminals.len(), 2, "one process per seed entry");
        for terminal in &terminals {
            assert!(app.terminal_runtimes.get(terminal).is_some());
            assert!(app.state.terminals.contains_key(terminal));
        }
        assert_eq!(
            pane_list_len(&mut app),
            panes_before,
            "dock panes are not API panes"
        );
        assert_eq!(app.state.workspaces.len(), workspaces_before);
        assert!(app.find_pane(seeded_pane(&app, "usage")).is_none());
        shut_down(&mut app);
    }

    #[tokio::test]
    async fn a_seeded_exit_respawns_and_a_user_exit_closes() {
        let mut app = test_app();
        app.init_dock(&seed(&[("usage", "sleep 5", 1)]), None);
        let usage = seeded_pane(&app, "usage");
        let first = live_terminals(&app);

        app.handle_internal_event(crate::events::AppEvent::PaneDied {
            pane_id: usage,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
        assert!(live_terminals(&app).is_empty(), "waiting to respawn");
        assert!(
            app.terminal_runtimes.get(&first[0]).is_none(),
            "the dead runtime is dropped"
        );
        let at = app.next_dock_deadline().expect("a respawn is scheduled");
        assert!(app.run_due_dock_respawns(at));
        let second = live_terminals(&app);
        assert_eq!(second.len(), 1);
        assert_ne!(second, first, "a fresh process");

        assert!(app.dock_split(usage, Direction::Vertical));
        let dock = app.state.dock.open().unwrap();
        let user = *dock
            .panes
            .iter()
            .find(|(_, pane)| pane.origin == DockPaneOrigin::User)
            .unwrap()
            .0;
        assert_eq!(live_terminals(&app).len(), 2);
        app.handle_internal_event(crate::events::AppEvent::PaneDied {
            pane_id: user,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
        let dock = app.state.dock.open().unwrap();
        assert_eq!(dock.panes.len(), 1, "the user pane closed with its program");
        assert_eq!(dock.layout.pane_count(), 1);
        shut_down(&mut app);
        assert!(matches!(app.state.dock.state, DockState::Closed));
    }

    #[tokio::test]
    async fn a_saved_dock_is_restored_and_beats_the_seed() {
        let mut app = test_app();
        app.init_dock(
            &seed(&[("usage", "sleep 5", 3), ("net", "sleep 5", 1)]),
            None,
        );
        let usage = seeded_pane(&app, "usage");
        assert!(app.dock_split(usage, Direction::Vertical));
        app.dock_set_rows(6);
        let snapshot = app.dock_snapshot().expect("an open dock is saved");
        let json = serde_json::to_string(&snapshot).unwrap();
        shut_down(&mut app);

        let saved: crate::persist::DockSnapshot = serde_json::from_str(&json).unwrap();
        let mut restored = test_app();
        restored.init_dock(&seed(&[("other", "sleep 5", 1)]), Some(&saved));
        let dock = restored.state.dock.open().expect("restored open");
        let mut labels = dock
            .panes
            .values()
            .map(|pane| pane.label().unwrap_or("user").to_owned())
            .collect::<Vec<_>>();
        labels.sort();
        assert_eq!(
            labels,
            vec!["net", "usage", "user"],
            "the seed was not applied"
        );
        assert_eq!(restored.state.dock.rows, 6);
        assert_eq!(
            live_terminals(&restored).len(),
            3,
            "every restored pane runs"
        );
        shut_down(&mut restored);

        let closed = crate::persist::DockSnapshot {
            layout: None,
            ..saved
        };
        let mut reopened = test_app();
        reopened.init_dock(&seed(&[("other", "sleep 5", 1)]), Some(&closed));
        assert!(
            matches!(reopened.state.dock.state, DockState::Closed),
            "a dock the user closed stays closed"
        );
    }

    #[tokio::test]
    async fn reload_config_leaves_a_running_dock_alone_and_builds_a_missing_one() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "herdr-dock-reload-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            "[[ui.dock.panes]]\nlabel = \"fresh\"\ncommand = \"sleep 5\"\n",
        )
        .unwrap();
        let previous = std::env::var_os(crate::config::CONFIG_PATH_ENV_VAR);
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut running = test_app();
        running.init_dock(&seed(&[("usage", "sleep 5", 1)]), None);
        let before = live_terminals(&running);
        let report = running.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(
            live_terminals(&running),
            before,
            "same processes after reload"
        );
        assert!(
            running.state.dock.seed.panes[0].label == "fresh",
            "the new seed is kept for reset"
        );
        running.reset_dock();
        let labels = running
            .state
            .dock
            .open()
            .unwrap()
            .panes
            .values()
            .map(|pane| pane.label().unwrap_or("user").to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec!["fresh"],
            "reset rebuilds from the reloaded seed"
        );
        shut_down(&mut running);

        let mut empty = test_app();
        empty.init_dock(&crate::config::DockConfig::default(), None);
        assert!(matches!(empty.state.dock.state, DockState::Unbuilt));
        empty.reload_config();
        assert_eq!(
            live_terminals(&empty).len(),
            1,
            "a missing dock is built on reload"
        );
        shut_down(&mut empty);

        match previous {
            Some(value) => std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, value),
            None => std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn agents_in_dock_panes_raise_no_list_entries_or_notifications() {
        let mut app = test_app();
        app.ensure_default_workspace();
        app.init_dock(&seed(&[("usage", "sleep 5", 1)]), None);
        let usage = seeded_pane(&app, "usage");
        assert!(app.dock_split(usage, Direction::Vertical));
        let user = *app
            .state
            .dock
            .open()
            .unwrap()
            .panes
            .iter()
            .find(|(_, pane)| pane.origin == DockPaneOrigin::User)
            .unwrap()
            .0;
        let now = Instant::now();
        app.handle_internal_event(crate::events::AppEvent::AgentProcessDetected {
            pane_id: user,
            agent: crate::detect::Agent::Claude,
            observed_at: now,
        });
        app.handle_internal_event(crate::events::AppEvent::StateChanged {
            pane_id: user,
            agent: Some(crate::detect::Agent::Claude),
            state: crate::detect::AgentState::Blocked,
            visible_blocker: true,
            visible_working: false,
            process_exited: false,
            observed_at: now,
        });

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "dock-agents".into(),
            method: crate::api::schema::Method::AgentList(Default::default()),
        });
        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        let crate::api::schema::ResponseResult::AgentList { agents } = parsed.result else {
            panic!("expected an agent list: {response}");
        };
        assert!(
            agents.is_empty(),
            "a dock pane is not listed as an agent: {agents:?}"
        );
        assert!(app.state.toast.is_none(), "no toast for a dock pane");
        assert!(
            app.state
                .next_pending_agent_notification_deadline()
                .is_none(),
            "no notification is queued for a dock pane"
        );

        // Positive control: the same event on a workspace pane is listed, so
        // the empty list above is the dock's doing, not the test's.
        let workspace_pane = app.state.workspaces[0].tabs[0].root_pane;
        app.handle_internal_event(crate::events::AppEvent::AgentProcessDetected {
            pane_id: workspace_pane,
            agent: crate::detect::Agent::Claude,
            observed_at: now,
        });
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "workspace-agents".into(),
            method: crate::api::schema::Method::AgentList(Default::default()),
        });
        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        let crate::api::schema::ResponseResult::AgentList { agents } = parsed.result else {
            panic!("expected an agent list: {response}");
        };
        assert_eq!(agents.len(), 1, "a workspace pane agent is listed");
        shut_down(&mut app);
    }

    #[tokio::test]
    async fn a_split_starts_in_the_source_panes_directory() {
        let mut app = test_app();
        app.init_dock(&seed(&[("usage", "sleep 5", 1)]), None);
        let usage = seeded_pane(&app, "usage");
        assert!(app.dock_split(usage, Direction::Horizontal));
        let dock = app.state.dock.open().unwrap();
        let user = dock
            .panes
            .values()
            .find(|pane| pane.origin == DockPaneOrigin::User)
            .unwrap();
        let tmp = std::fs::canonicalize("/tmp").unwrap();
        assert_eq!(std::fs::canonicalize(&user.cwd).unwrap(), tmp);
        shut_down(&mut app);
    }

    #[tokio::test]
    async fn session_saves_carry_the_dock() {
        let mut app = test_app();
        app.ensure_default_workspace();
        app.init_dock(&seed(&[("usage", "sleep 5", 1)]), None);
        let snapshot = app.dock_snapshot().unwrap();
        assert_eq!(snapshot.rows, 3);
        assert_eq!(snapshot.panes.len(), 1);
        let saved = snapshot.panes.values().next().unwrap();
        assert_eq!(saved.label.as_deref(), Some("usage"));
        assert_eq!(saved.command.as_deref(), Some("sleep 5"));
        shut_down(&mut app);
    }

    // -----------------------------------------------------------------------
    // Pane stacks in the dock (spec 002)
    // -----------------------------------------------------------------------

    /// `usage` with a user shell stacked over it, both started.
    fn stacked_dock() -> (Dock, PaneId, PaneId) {
        let t0 = Instant::now();
        let (mut dock, spawns) = super::tests::seeded_dock(&[("usage", "hayami", 1)]);
        let seeded = spawns[0].pane_id;
        super::tests::start_all(&mut dock, &spawns, t0);
        let added = dock
            .stack_new(seeded, PathBuf::from("/tmp"))
            .expect("a stacked dock pane");
        dock.started(added.pane_id, TerminalId::alloc(), t0);
        (dock, seeded, added.pane_id)
    }

    #[test]
    fn a_dock_stack_keeps_its_slot_and_draws_a_strip_row() {
        let (dock, seeded, added) = stacked_dock();
        let layout = dock.open().unwrap();
        assert_eq!(layout.layout.pane_count(), 1, "no new slot");
        assert_eq!(layout.panes.len(), 2);
        assert!(dock.is_hidden(seeded) && !dock.is_hidden(added));
        let geometry = dock.geometry();
        assert_eq!(geometry.len(), 1);
        let strip = geometry[0].strip.expect("the visible member has a strip");
        assert_eq!(strip.y, 0);
        assert_eq!(geometry[0].inner.y, 1, "the program starts below the strip");
        let sizes = dock.pane_sizes();
        assert_eq!(
            sizes[&seeded], sizes[&added],
            "the hidden member is sized alike"
        );
        assert_eq!(sizes[&added].0, dock.rows - 1);
    }

    #[test]
    fn dock_stacks_cycle_focus_and_hand_the_slot_on_when_closed() {
        let (mut dock, seeded, added) = stacked_dock();
        assert!(dock.stack_cycle(added, true));
        assert!(!dock.is_hidden(seeded) && dock.is_hidden(added));
        assert!(dock.focus(added), "focusing a hidden member shows it");
        assert!(!dock.is_hidden(added));

        assert!(dock.close(added).is_some());
        let layout = dock.open().unwrap();
        assert_eq!(
            layout.layout.pane_ids(),
            vec![seeded],
            "the seeded pane shows again"
        );
        assert!(layout.stacks.is_empty());
        assert!(!dock.toggle_zoom(), "one pane left cannot zoom");
    }

    #[test]
    fn dock_stacks_are_saved_restored_and_their_hidden_members_started() {
        let (dock, _seeded, _added) = stacked_dock();
        let snapshot = dock.to_snapshot().expect("an open dock is saved");
        assert_eq!(snapshot.stacks.len(), 1);
        let mut restored = Dock {
            width: 100,
            ..Dock::default()
        };
        restored.restore(&snapshot);
        let layout = restored.open().expect("restored open");
        assert_eq!(layout.stacks.len(), 1);
        assert_eq!(layout.panes.len(), 2);
        let visible = layout.stacks[0].visible().unwrap();
        assert_eq!(layout.layout.pane_ids(), vec![visible]);
        assert_eq!(
            layout.panes[&visible].label(),
            None,
            "the user shell stays the visible member"
        );
        assert_eq!(
            restored.spawn_pending().len(),
            2,
            "hidden members start too"
        );

        let mut damaged = snapshot.clone();
        damaged.stacks[0].panes.push(999);
        damaged.stacks.push(crate::persist::StackSnapshot {
            panes: vec![424242, 1],
            active: 0,
        });
        let mut repaired = Dock {
            width: 100,
            ..Dock::default()
        };
        repaired.restore(&damaged);
        assert_eq!(
            repaired.open().unwrap().stacks.len(),
            1,
            "repaired, not failed"
        );
    }
}
