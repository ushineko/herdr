# 002 — herdr nested tabs: several panes in one layout slot

> Personal fork patch. Not part of herdr; not affiliated with the herdr project.

## Status: INCOMPLETE

## Context

A herdr tab lays its panes out as a split tree, and every pane takes screen
space. Supporting tools such as yazi and lazygit do not need much room, but
today each one needs its own split (a per-tab panel script),
which crowds every tab. Moving them to their own top-level tab hides them
from the main view.

**Nested tabs** (slot tabs) let one layout slot hold several panes: one
visible, the others running behind a one-row tab strip. Switching tabs
changes only that slot, so the rest of the layout stays where it is. This is
upstream's #3775 ("stacked pane groups / pane-local tabs") and #529, both
closed under the bugs-only issue policy, not rejected on merit. As with the
spaces bar and the dock, this stays a local patch, never an upstream PR.

Base: branch `ushineko/dock` (v0.9.5, the spaces bar, then the dock, spec 001). Slot tabs are a separate commit series on top,
so they can be dropped alone.

Related: fixed-size pane pinning, which spec 001 deferred, is now planned as
spec `003`.

## Requirements

Decided with the user on 2026-11-19:

1. **Slots hold tabs.** A layout slot can hold several panes. One is visible;
   the others keep running, keeping their scrollback, cwd, title and agent
   state. Agent detection and notifications continue for hidden tabs.
2. **Strip:** its own row at the top of the slot, shown only while the slot
   holds two or more tabs. A single pane looks exactly as it does today.
3. **Creating:** a key opens a new shell as another tab in the focused slot
   ("new tab in this slot"). This is the only way into a stack in v1.
4. **Saved sessions are rollback-safe.** The saved layout tree keeps each
   slot's visible pane in its place; an optional side field lists the hidden
   tabs. Stock herdr restores the visible panes and drops the hidden ones,
   without failing.
5. **Scope:** workspace tabs and the dock (spec 001) alike.
6. **No second multiplexer.** Only herdr itself.

### Behaviour

- **Labels:** each strip entry shows the pane's label, else its agent name,
  else its process; the visible tab is highlighted. A hidden tab whose agent
  is blocked or done shows that state's glyph, so attention needs are not
  hidden behind the strip.
- **Switching:** click a strip entry; `keys.next_slot_tab` (proposed
  `prefix+.`) and `keys.previous_slot_tab` (proposed `prefix+,`) cycle within
  the focused slot; the wheel over the strip cycles too. The newly visible
  tab takes focus.
- **New tab:** `keys.new_slot_tab` (proposed `prefix+t`) starts the default
  shell in the focused pane's cwd, as the last tab of the focused slot, and
  makes it visible.
- **Closing:** closing or exiting the visible tab shows its neighbour (the
  next tab, else the previous one). When one tab is left, the strip goes
  away and the slot is a plain pane again.
- **Moving tabs:** split, swap, move-to-tab and zoom act on the slot as a
  whole; zoom shows the visible tab. The strip stays visible while zoomed.

### Config (proposed)

```toml
[keys]
new_slot_tab = "prefix+t"        # checked free by the keymap test
next_slot_tab = "prefix+."
previous_slot_tab = "prefix+,"
```

No other config. The strip draws in the pane border colours (`overlay0`,
with `accent` for the visible tab while the slot is focused).

## Design

### Model: hidden tabs stay in the tab, out of the tree

- Each `Tab` (and the dock's layout) keeps its `TileLayout`, whose leaf for a
  stacked slot is the **visible** tab's pane. The hidden tabs are recorded
  beside it:
  `stacks: Vec<SlotStack { panes: Vec<PaneId>, active: usize }>`.
- Switching tabs replaces that leaf's pane id in place
  (`TileLayout::replace_pane`, new). Ratios, splits and every other slot are
  untouched.
- Hidden panes stay in `Tab::panes` with their runtimes. They remain real
  panes in a real workspace, so agent detection, notifications, `pane list`,
  `agent list`, Go To and pane ids all keep working. This is the reason not to
  keep them in a separate store, which would hide their agents.

### The invariant change (the main risk)

Upstream asserts that a tab's `panes` equal its layout's leaves
(`Workspace::assert_invariants_for_test`). That becomes "`panes` equal the
leaves plus the hidden stack members". About 44 non-test sites call
`layout.pane_ids()` or `layout.pane_count()`, and more enumerate
`tab.panes`. Each needs a decision: visible panes only (geometry, rendering,
focus by direction) or all panes (process lifecycle, agent lists, close-tab,
session save, handoff).

**Phase 0 is that audit.** Classify every enumeration site, record the
classification in this spec with file references, and find any site where a
hidden pane would misbehave: resized to zero, missed on tab close, lost on
handoff, focused while invisible. If the audit finds the change unsafe to
contain, implementation stops and the user decides.

### Geometry and rendering

- The server draws the strip itself, inside the slot's area: first row strip,
  remaining rows pane. The pane's rect in the pane surface excludes the
  strip, so clients and pane programs see a slightly shorter pane, and the
  frozen bincode `PaneSurfaceFrame` does not change. A stock client therefore
  sees the strip too, though it cannot click it.
- Hidden tabs are sized to the visible tab's content rect, as background tabs
  are, so switching does not reflow.
- A slot shorter than two rows draws no strip and shows only the visible tab.

### Protocol (same gate as the dock)

- The server advertises a `slot_tabs` capability. It sends strip geometry
  (per slot: strip rect, and tab pane ids with labels) as a named control,
  `endpoint.slot-tabs.v1`, alongside the snapshot.
- Clients send switch, new and close-tab operations as
  `endpoint.slot-tabs.control.v1`, only when the capability is present. The
  keys work through the same controls. A client that never opts in gets no
  slot-tab controls.

### Persistence

- `TabSnapshot` gains an optional `stacks` field: for each stacked slot, the
  ordered saved pane ids and the active index. Hidden panes are also saved in
  the existing `panes` map (cwd, label, agent session), so restore can
  respawn them. Agents resume through the existing agent-resume path.
- The snapshot version stays 3. Stock herdr restores the tree (visible
  panes) and ignores `stacks` and the extra `panes` entries. Phase 0 confirms
  that, as spec 001 did for the dock.
- The dock's saved layout gets the same optional field.

### Live handoff

Handoff transfers runtimes listed by the workspace snapshot. Hidden panes are
in `Tab::panes`, so they should transfer like any pane. Phase 0 verifies this
in the audit.

## Acceptance Criteria

### Phase 0 — audit and compatibility
- [ ] Every non-test site that enumerates `layout.pane_ids()`,
      `layout.pane_count()` or `Tab::panes` is classified as visible-only or
      all-panes in this spec, with file references, and every site where a
      hidden pane would misbehave is listed with its fix.
- [ ] A test shows that stock v0.9.5 restores a session containing stacks
      (the visible panes, no error), as for the dock.
- [ ] Live handoff carries hidden panes, or the gap is recorded with a
      decision.

### Model and lifecycle
- [ ] `new_slot_tab` adds a shell (default shell, cwd of the focused pane) as
      the last tab of the focused slot and shows it. The rest of the layout
      (rects of every other pane) is unchanged.
- [ ] Switching tabs changes only the slot's visible pane. Hidden panes keep
      running, are sized to the slot's content rect, and keep their
      scrollback.
- [ ] Closing or exiting the visible tab shows its neighbour. When one tab is
      left, the slot is a plain pane and draws no strip.
- [ ] Split, swap, move-to-tab and zoom act on the whole slot.
- [ ] Hidden tabs appear in `pane list`, `agent list`, Go To and
      notifications, like any pane.
- [ ] The workspace invariant check is updated, and passes in every existing
      test plus the new ones.

### Strip and input
- [ ] A slot with two or more tabs draws a one-row strip at its top: labels
      (pane label, else agent, else process), the visible tab highlighted, a
      blocked or done hidden agent shown by its glyph. A single pane is drawn
      exactly as before.
- [ ] Clicking a strip entry, `next_slot_tab` / `previous_slot_tab`, and the
      wheel over the strip switch tabs in that slot and focus the new tab.
- [ ] `keys.new_slot_tab`, `keys.next_slot_tab` and `keys.previous_slot_tab`
      exist, and the shipped defaults produce no keymap diagnostics.
- [ ] A stock client connected to a patched server renders the strip and
      works; a patched client against a stock server shows no slot-tab
      controls.

### Dock
- [ ] Slot tabs work in dock slots: `new_slot_tab` with the dock focused adds
      a tab to the focused dock slot, and the dock's saved layout keeps them.

### Persistence
- [ ] A session with stacks saves and restores with the same tabs, order,
      visible tab and pane cwds. Agents in hidden tabs resume through the
      existing agent-resume path.

### Integration (real downstream systems)
- [ ] An integration test starts a real server and client, creates a slot
      tab, types into each tab, switches, and asserts that each pane kept its
      own output and that `pane list` reports both.
- [ ] An integration test restarts the server and asserts the stack is
      restored, with the same visible tab.

## Test strategy (for review before tests are written)

- **Unit (layout):** `TileLayout::replace_pane` keeps ratios and paths;
  strip-row geometry; slots too short for a strip.
- **Unit (model):** stack create, switch, close, exit, last tab left, and
  split, swap, move and zoom of a stacked slot; the invariant check with
  hidden panes.
- **Server tests** with test runtimes, as for the dock: the strip drawn into
  the tab surface, the visible pane's rect below the strip, hidden-pane
  sizing, and agent state of hidden tabs reaching agent lists and
  notifications.
- **Client-shell tests:** strip clicks, keys and wheel turned into controls.
- **Compatibility:** stock v0.9.5 against patched builds in both directions,
  and a stock restore of a session with stacks.
- **Integration:** the two real server-plus-client tests above.
- **Mock boundaries:** none for terminals, the protocol or session files.
- **Regression guard:** each behavioural test is checked to fail with its
  behaviour removed.

## Risks & Assumptions

- **Invariant change (high):** this is the first local patch to change a
  core workspace invariant, not add beside it. A missed enumeration site can
  hide a pane from close-tab, lose it on handoff, or resize it to zero.
  Phase 0 is the mitigation and the stop point.
- **Rebase cost (high):** it touches workspace, layout, persistence and the
  renderer. Each upstream release will cost more to port than the spaces bar
  or the dock. It is a separate series so it can be dropped.
- **Rollback:** the side-field design means rollback loses the hidden tabs'
  panes (their processes end) but keeps the session. Same pattern as the
  dock.
- **Stock clients** see the strip but cannot click it, and their keymaps lack
  the new actions. Accepted.
- **Assumption:** the user's panel use case is served by creating tabs by
  hand, with `prefix+t` then starting the tool. v1 has no CLI to build a
  stack, so the panel script cannot build a tabbed slot itself (see
  below).

## Deliberately not supported (v1)

- Stacking an existing neighbouring pane into a slot, or unstacking a tab
  back into its own split.
- A CLI or API to build stacks. As a result, scripts such as
  the panel script cannot set up a tabbed panel. This is the most
  likely follow-up.
- Dragging tabs between slots, or reordering them within a strip.
- Per-tab names separate from pane names (the strip uses the pane label).
- Strip placement other than the top row.
- Pinning a slot to a fixed size (spec 003).

## E2E Test Plan

Environment: the development machine (Linux x86_64). Steps 1–5 run on a throwaway `--session` driven
through a private `tmux -L` harness, which is test tooling, not part of the
user's setup. Step 6 runs live after the user's desk test.

1. In a split layout, `prefix+t` in the right pane → a strip appears in that
   slot with two entries, the new shell is visible and focused, and the left
   pane's rect is unchanged. *(Create; layout untouched.)*
2. Run `yazi` in tab 1 and `lazygit` in tab 2, then switch with `prefix+.`,
   `prefix+,`, a strip click and the wheel → each tool redraws intact; no
   reflow. *(Switch; hidden sizing.)*
3. Start an agent in a hidden tab and let it block → its glyph shows in the
   strip, and `herdr agent list` shows it. *(Hidden agents.)*
4. `exit` in the visible tab → its neighbour shows; with one tab left, the
   strip disappears. *(Close; collapse.)*
5. Restart the server → the stack, its order and visible tab are restored;
   attach a stock v0.9.5 client → the strip renders. *(Persistence;
   compatibility.)*
6. Live: replace the yazi/lazygit splits in daily use with one tabbed slot
   for a day. *(Real use.)*

### Prod visibility

Not applicable. This is a personal workstation tool with no production
deployment. Observability is herdr's server log; slot-tab creation, switching
and closing are logged with pane ids.

## Alternatives Considered

- A new layout node type for stacks: rejected by the user. Stock herdr could
  not load such a session, so rollback would lose the whole layout.
- Keeping hidden tabs in a store separate from `Tab::panes`, as the dock does:
  rejected, because hidden agents would vanish from agent lists and
  notifications.
- The strip in the pane border title: rejected by the user, since it would
  exist only while borders are drawn.
- tmux or another multiplexer inside a pane: rejected by the user, because
  herdr is the only session layer.
