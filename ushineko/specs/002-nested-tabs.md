# 002 — herdr nested tabs (pane stacks): several panes in one layout slot

> Personal fork patch. Not part of herdr; not affiliated with the herdr project.

## Status: INCOMPLETE

## Context

A herdr tab lays its panes out as a split tree, and every pane takes screen
space. Supporting tools such as yazi and lazygit do not need much room, but
today each one needs its own split (a per-tab panel script),
which crowds every tab. Moving them to their own top-level tab hides them
from the main view.

**Nested tabs** let one layout slot hold several panes: one visible, the
others running behind a one-row strip. Switching changes only that slot, so
the rest of the layout stays where it is. This is upstream's #3775 ("stacked
pane groups / pane-local tabs") and #529, both closed under the bugs-only
issue policy, not rejected on merit. As with the spaces bar and the dock,
this stays a fork patch, never an upstream PR.

### Terms

herdr already uses "tab" for the tabs of a workspace, so the code and config
do not call these tabs:

- **Pane stack** (or stack): a layout slot holding two or more panes.
- **Stacked pane**: one member of a stack. The **visible pane** is the one
  in the layout tree; the others are **hidden panes**.
- **Stack strip**: the one-row bar at the top of a stack.

User-facing docs may still say "nested tabs" in prose.

Base: branch `ushineko/dock` (v0.9.5, the spaces bar, then the dock, spec
001). Pane stacks are a separate commit series on `ushineko/pane-stacks`, so
they can be dropped alone.

Related: fixed-size pane pinning, which spec 001 deferred, is planned as spec
`003`.

## Requirements

Decided with the user on 2026-11-19:

1. **Slots hold stacks.** A layout slot can hold several panes. One is
   visible; the others keep running, keeping their scrollback, cwd, title and
   agent state. Agent detection and notifications continue for hidden panes.
2. **Strip:** its own row at the top of the slot, shown only while the slot
   holds two or more panes. A single pane looks exactly as it does today.
3. **Creating:** a key opens a new shell as another pane in the focused
   slot's stack. This is the only way into a stack in v1.
4. **Saved sessions are rollback-safe.** The saved layout tree keeps each
   slot's visible pane in its place; an optional side field lists the hidden
   panes. Stock herdr restores the visible panes and drops the hidden ones,
   without failing.
5. **Scope:** workspace tabs and the dock (spec 001) alike.
6. **No second multiplexer.** Only herdr itself.
7. **Scriptable from outside.** Hidden panes are ordinary panes to the JSON
   API, so herdr-resurrect and other scripts can find, read and drive them
   (added 2026-11-19 from the user's planned test: herdr-resurrect ported to
   the fork, with yazi and lazygit in one stack and hayami in the dock).

### Behaviour

- **Labels:** each strip entry shows the pane's label, else its agent name,
  else its process; the visible pane is highlighted. A hidden pane whose
  agent is blocked or done shows that state's glyph, so attention needs are
  not hidden behind the strip.
- **Switching:** click a strip entry; `keys.stack_next` (proposed `prefix+.`)
  and `keys.stack_previous` (proposed `prefix+,`) cycle within the focused
  slot; the wheel over the strip cycles too. The newly visible pane takes
  focus.
- **New stacked pane:** `keys.stack_new_pane` (proposed `prefix+t`) starts the
  default shell in the focused pane's cwd, as the last member of the focused
  slot's stack (making one if needed), and shows it.
- **Closing:** closing or exiting the visible pane shows its neighbour (the
  next pane, else the previous one). When one pane is left, the strip goes
  away and the slot is a plain pane again.
- **Moving:** split, swap, move-to-tab and zoom act on the slot as a whole;
  zoom shows the visible pane. The strip stays visible while zoomed.
- **Focusing a hidden pane from outside** (Go To, an agent-list jump, a
  notification click, `herdr pane focus`) first makes it the visible pane of
  its stack.
- **Geometry commands on a hidden pane** (split, resize, swap, zoom by pane
  id, from keys or the API) reveal it first, then act on its slot. (Decided
  2026-11-19.)
- **Notifications:** a hidden pane counts as not visible, even in the active
  tab. Its agent's blocked and done states notify as a background tab's would,
  and are not marked seen until the pane is shown. (Decided 2026-11-19.)

### Config (proposed)

```toml
[keys]
stack_new_pane = "prefix+t"      # checked free by the keymap test
stack_next = "prefix+."
stack_previous = "prefix+,"
```

No other config. The strip draws in the pane border colours (`overlay0`,
with `accent` for the visible pane while the slot is focused).

## Design

### Model: hidden panes stay in the tab, out of the tree

- Each `Tab` (and the dock's layout) keeps its `TileLayout`, whose leaf for a
  stacked slot is the **visible** pane. The hidden panes are recorded beside
  it: `stacks: Vec<PaneStack { panes: Vec<PaneId>, active: usize }>`.
- Switching replaces that leaf's pane id in place (`TileLayout::replace_pane`,
  new). Ratios, splits and every other slot are untouched.
- Hidden panes stay in `Tab::panes` with their runtimes. They remain real
  panes in a real workspace, so agent detection, notifications, `pane list`,
  `agent list`, Go To and pane ids all keep working. This is the reason not to
  keep them in a separate store, which would hide their agents.

### The invariant change (the main risk)

Upstream asserts that a tab's `panes` equal its layout's leaves
(`Workspace::assert_invariants_for_test`). That becomes "`panes` equal the
leaves plus the hidden stack members, every stack has two or more members,
its visible member is a leaf, and no pane is in two stacks". About 44
non-test sites call `layout.pane_ids()` or `layout.pane_count()`, and more
enumerate `tab.panes`. Each needs a decision: visible panes only (geometry,
rendering, focus by direction) or all panes (process lifecycle, agent lists,
close-tab, session save, handoff).

herdr's `AGENTS.md` classifies this as refactor-risk work (workspace and pane
identity, persisted state, restore and handoff). Its rules apply: name or add
characterization tests for the protected behaviour before moving code, and
check identity changes with `Workspace::assert_invariants_for_test()` and
`AppState::assert_invariants_for_test()` against the adversarial fixtures
(`Workspace::test_adversarial_identity_state()`,
`AppState::test_with_adversarial_identity_state()`), extended with stacks.
An independent review of the Phase 0 audit stands in for upstream's
roundtable.

**Phase 0 is that audit.** Classify every enumeration site, record the
classification in this spec with file references, and find any site where a
hidden pane would misbehave: resized to zero, missed on tab close, lost on
handoff, focused while invisible. If the audit finds the change unsafe to
contain, implementation stops and the user decides.

### Geometry and rendering

- The server draws the strip itself, inside the slot's area: first row strip,
  remaining rows pane. **Why server-side**, although the 0.9 shell chrome is
  drawn by the client: the strip lives inside the tab surface, where the
  server already draws split borders (`src/server/render_stream.rs`). Drawing
  it there keeps the client's pane geometry as the server sends it, needs no
  new client-side layout projection, and lets stock clients show it. The
  cost: strip changes travel in pane-surface frames, and stock clients cannot
  click it.
- The visible pane's rect in the pane surface excludes the strip, so clients
  and pane programs see a slightly shorter pane, and the frozen bincode
  `PaneSurfaceFrame` does not change.
- Hidden panes are sized to the visible pane's content rect, as panes in
  background tabs are, so switching does not reflow.
- A slot shorter than two rows draws no strip and shows only the visible pane.
- **Render hot path** (`AGENTS.md`): hidden panes still parse output, but
  their output must not trigger presentation work. The hidden-source early
  exit (`pty_sources_visible_to_any_render_target`, via
  `any_shell_surface_contains_pane`) checks `tab.panes`, not the layout
  (Phase 0), so it would count hidden members as visible. It must check
  stack visibility, and a test pins that. A strip entry's agent glyph changes through the normal
  agent-state redraw, not through pty output.

### Protocol (same gate as the dock)

- The server advertises a `pane_stacks` capability. It sends strip geometry
  (per stack: strip rect, and member pane ids with labels and the visible
  index) as a named control, `endpoint.pane-stack.v1`, alongside the snapshot.
- Clients send switch, new and close operations as
  `endpoint.pane-stack.control.v1`, only when the capability is present. The
  keys work through the same controls. A client that never opts in gets no
  pane-stack controls.
- Serde rules, as for the dock: every new JSON field is optional with a
  default; enums for operations end in `#[serde(other)] Unknown`, which the
  server ignores; no frozen bincode message, `ClientShellSnapshot` or endpoint
  fixture changes.

### JSON API

- `pane list` keeps listing every pane, hidden ones included. Each stacked
  pane gains an optional `stack` object: `{ "stack_id", "index", "visible" }`.
  Unstacked panes omit it, so existing consumers see no change.
- `pane read`, `pane process-info`, `pane run` and the agent methods work on
  hidden panes as on any pane (they are not geometry operations).
- No API to build or rearrange stacks in v1 (see Deliberately not supported).

### Persistence

- `TabSnapshot` gains an optional `stacks` field: for each stack, the ordered
  saved pane ids and the active index. Hidden panes are also saved in the
  existing `panes` map (cwd, label, agent session), so restore can respawn
  them. Agents resume through the existing agent-resume path.
- The snapshot version stays 3. Stock herdr restores the tree (visible panes)
  and ignores `stacks` and the extra `panes` entries. Phase 0 confirms that,
  as spec 001 did for the dock.
- Restore is lenient: a stack entry naming an unknown pane, a stack with
  fewer than two known members, or an active index out of range is repaired
  (members dropped, index clamped) and logged, never a failed restore.
- The dock's saved layout gets the same optional field.

### Phase 0 findings

**Stock restore (2026-11-19).** A stock v0.9.5 release build saved a real
session in an isolated config (workspace `marker-ws`, panes 1 and 2 split
side by side). The saved tab was then edited to add pane 3 (a hidden member,
cwd `/usr`) to the `panes` map and
`"stacks": [{"panes": [2, 3], "active": 0}]`. Stock v0.9.5 restored it:
`session restore evaluated outcome="ok" workspaces=1`, no warnings, panes 1
and 2 back with their cwds, pane 3 not created. Its next save dropped
`stacks` and pane 3. A second variant also set the tab's `focused` to the
hidden pane 3 (with `active: 1`); stock restored it the same way and focused
pane 1. Rollback therefore loses only the hidden panes, as designed.

**Enumeration audit (2026-11-19)**, full table in
[`002-phase0-audit.md`](002-phase0-audit.md): 89 classified rows, 30 needing
change, plus 4 sites handling hidden pane ids. Spot-checked by hand:
`any_shell_surface_contains_pane`, the strict handoff import and the
last-pane checks. No site makes the change unsafe to contain; every fix is a
local substitution or a small stack-aware helper (`is_visible`, `reveal`,
`replace_pane`, size-hidden-like-visible). By theme:

- **A. "Last pane" uses the leaf count (critical).** `Workspace::remove_pane`
  (`src/workspace.rs`), `Tab::detach_pane` (`src/workspace/tab.rs`) and close
  actions (`src/app/actions.rs`) test `layout.pane_count() <= 1`. In a
  one-slot stack, closing, moving or the death of any member would remove the
  tab, or the whole workspace, and leave the other members' processes running
  with no pane. Fix: "last pane" means `tab.panes.len()`, and removing the
  visible member hands its leaf to the next member (`replace_pane`).
- **B. Lists built from the layout:** the session snapshot that feeds the
  client's Go To and notification activation, `agent list`, agent lookup by
  name, `pane list` and the sidebar agent rows would all omit hidden panes.
- **C. Lifecycle tied to geometry:** deferred agent resume, worktree-removal
  shutdown, plugin-record cleanup, and spawning and restoring dock members.
- **D. Persistence and handoff:** restore and the handoff import walk the
  tree only. The import fails if any imported runtime is left unused, so
  until restore learns stacks every handoff would fail and roll back. It fails
  safely: the old server keeps running.
- **E. Render hot path:** see Geometry and rendering (corrected above).
- **F. Sizing:** nothing resizes hidden members. They keep a stale size (never
  zero) and would reflow when shown. Fix in `resize_tab_panes` and
  `compute_pane_infos_for_tab`.
- **G. Focus and visibility semantics:** focusing a hidden pane by API does
  nothing but reports success; geometry API calls on a hidden pane return
  "pane not found"; hidden members of the active tab are treated as seen,
  which suppresses their notifications. Decisions are listed under Open
  decisions.

**Independent review (2026-11-19)**, in
[`002-phase0-review.md`](002-phase0-review.md): the audit's table is complete
and themes A, D and E hold at the cited lines. It added:

- **Notifications compute "active tab" inline** in
  `src/server/headless/notifications.rs` (sound and toast paths), so fixing
  only the shared helper would still suppress hidden panes there.
  (Confirmed by hand.)
- **`pane move` to a hidden target** takes the source pane, fails the insert,
  and recovers it into a new tab; split and swap by a hidden id no-op or
  error. Reveal-before-act belongs in the API layer, not only the layout.
- **Dock hidden members:** `dock_sources_dirty` would redraw and re-encode
  the dock on a hidden dock member's output.
- **Client visibility:** once hidden panes are listed in the client snapshot,
  client pane cycling and copy-mode cannot tell them apart. Decided
  2026-11-19: patched clients derive the hidden set from the
  `endpoint.pane-stack.v1` control; `ClientShellPane` and the frozen
  `tests/fixtures/endpoint-snapshot-v1.json` stay unchanged.
- **Smaller:** `replace_pane` must carry focus and previous focus; restore
  re-roots a tab whose saved `root_pane` is hidden; public pane numbers for
  hidden panes on restore are a prerequisite for the list fixes; a stack
  switch must bump the client projection revision so the retained-surface
  planner falls back.
- **Pre-existing, from spec 001:** dock pane output always counts as visible
  to `pty_source_visible_to_render_targets`, because `find_pane` knows only
  workspace panes. This skews the hidden-source benchmark and is fixed with
  the dock's hidden-member case.

No finding needs a model redesign.

Unverified: whether stock herdr keeps saved scrollback after a rollback (the
history fingerprint may no longer match).

### Live handoff

Handoff transfers runtimes listed by the workspace snapshot. Hidden panes are
in `Tab::panes`, so they should transfer like any pane. Phase 0 verifies this
in the audit.

## Acceptance Criteria

### Phase 0 — audit and compatibility
- [x] Every non-test site that enumerates `layout.pane_ids()`,
      `layout.pane_count()` or `Tab::panes` is classified as visible-only or
      all-panes in this spec, with file references, and every site where a
      hidden pane would misbehave is listed with its fix.
      ([`002-phase0-audit.md`](002-phase0-audit.md); summary under Phase 0
      findings.)
- [x] Characterization tests for the protected behaviour (close-tab, session
      save and restore, handoff transfer, directional focus, agent lists) are
      named or added before code moves. (Named in the audit, section 6. None
      covers a tab with more panes than layout leaves; those are new tests,
      written first in implementation.)
- [x] A test shows that stock v0.9.5 restores a session containing stacks
      (the visible panes, no error), as for the dock. (Phase 0 findings.)
- [x] Live handoff carries hidden panes, or the gap is recorded with a
      decision. Export already sends hidden runtimes; the import must learn
      stacks (implementation criterion below). Handoff *to stock* herdr fails
      safely while stacks exist; decided 2026-11-19: rollback to stock is a
      cold restart, documented.
- [x] The audit has an independent review before implementation starts.
      ([`002-phase0-review.md`](002-phase0-review.md); summary under Phase 0
      findings.)

### Model and lifecycle
- [ ] `stack_new_pane` adds a shell (default shell, cwd of the focused pane)
      as the last member of the focused slot's stack and shows it. The rest of
      the layout (rects of every other pane) is unchanged.
- [ ] Switching changes only the slot's visible pane. Hidden panes keep
      running, are sized to the slot's content rect, and keep their
      scrollback.
- [ ] Closing or exiting the visible pane shows its neighbour. When one pane
      is left, the slot is a plain pane and draws no strip.
- [ ] Split, swap, move-to-tab and zoom act on the whole slot.
- [x] Focusing a hidden pane from Go To, an agent-list jump, a notification
      or `herdr pane focus` makes it visible first. (Part 3: `focus_pane_in_workspace` shows it; `focusing_a_hidden_member_shows_it`.)
- [x] Hidden panes appear in agent lists, Go To and notifications, like any
      pane, and their notifications (sound and toast paths included) fire
      even when their tab is active. (Part 3: lists use `Tab::all_pane_ids`; `pane_is_in_active_tab` and both inline checks in `notifications.rs` treat hidden members as off screen; `a_hidden_agent_is_listed`, `hidden_members_count_as_off_screen_for_notifications_and_seen`.)
- [ ] Patched clients learn hidden panes from the pane-stack control: pane
      cycling and copy-mode never land on a hidden pane. `ClientShellPane`
      and the endpoint fixtures are unchanged.
- [x] Geometry commands on a hidden pane reveal it, then act on its slot,
      in the API layer too: `pane move` with a hidden target, and split and
      swap by a hidden id (tests). (Part 3: `App::reveal_geometry_targets` before dispatch; `geometry_requests_on_a_hidden_member_reveal_it_then_act`.)
- [x] Switching the visible pane carries focus and previous focus, and bumps
      the client projection revision. (Parts 1 and 3: `replace_pane`; `App::stack_layout_changed` requests a full render.)
- [x] Closing, moving or the exit of any member of a one-slot stack removes
      only that pane; the tab and workspace stay (audit theme A, tested for
      each close, move and death path). (Part 1:
      `a_member_of_a_one_slot_stack_is_not_the_last_pane`,
      `closing_a_member_of_a_one_slot_stack_needs_no_workspace_confirmation`.)
- [ ] Live handoff between two patched builds carries stacks: same members,
      visible pane and processes (integration test).
- [x] The workspace and app invariant checks are extended for stacks, and
      pass in every existing test, the new ones, and against the adversarial
      identity fixtures extended with stacks. (Part 1:
      `Tab::assert_stack_invariants_for_test`,
      `Workspace::test_adversarial_identity_state_with_stacks`.)

### JSON API
- [x] `pane list` lists hidden panes, with the optional `stack` object on
      stacked panes only; unstacked output is unchanged. (Part 3: `PaneInfo.stack`; `pane_list_reports_hidden_members_with_their_stack`.)
- [x] `pane read`, `pane process-info` and `pane run` work on a hidden pane. (Part 3: they address panes by id, not geometry; `a_hidden_member_can_be_read_without_being_shown`. `pane run` is checked end to end in part 5.)

### Strip and input
- [ ] A slot with two or more panes draws a one-row strip at its top: labels
      (pane label, else agent, else process), the visible pane highlighted, a
      blocked or done hidden agent shown by its glyph. A single pane is drawn
      exactly as before.
- [ ] Clicking a strip entry, `stack_next` / `stack_previous`, and the wheel
      over the strip switch panes in that slot and focus the new one.
- [ ] `keys.stack_new_pane`, `keys.stack_next` and `keys.stack_previous`
      exist, and the shipped defaults produce no keymap diagnostics.
- [ ] A stock client connected to a patched server renders the strip and
      works; a patched client against a stock server shows no pane-stack
      controls.
- [x] Unknown pane-stack operations and malformed controls are ignored by the
      server (serde `Unknown` fallbacks, tested). (Part 3: `pane_stack_controls_round_trip_and_unknown_ops_decode_as_unknown`, `show_and_cycle_controls_switch_the_slot_and_unknown_ones_are_ignored`.)

### Render hot path
- [x] Output from a hidden pane takes the hidden-source early exit (test),
      for workspace stacks and dock stacks alike. (Part 3, workspace stacks: `hidden_member_output_takes_the_hidden_source_early_exit`. Dock stacks: part 5.)
- [ ] `just bench-render-scale` with 1 and 15 populated panes, some of them
      hidden in stacks, compared with `ushineko/dock`; the delta is recorded
      here.

### Dock
- [ ] Stacks work in dock slots: `stack_new_pane` with the dock focused adds a
      pane to the focused dock slot's stack, and the dock's saved layout keeps
      them.

### Persistence
- [x] A session with stacks saves and restores with the same members, order,
      visible pane and pane cwds. Agents in hidden panes resume through the
      existing agent-resume path. (Part 2:
      `a_saved_stack_restores_with_members_order_visible_pane_and_numbers`,
      `capture_saves_stacks_and_hidden_panes_and_omits_an_empty_field`,
      `hidden_stack_members_are_resume_candidates_at_the_visible_size`.)
- [x] A saved stack with unknown members or an out-of-range index restores
      repaired, without error (test). A tab whose saved `root_pane` is a
      hidden member keeps its identity; hidden panes keep their public pane
      numbers. (Part 2: `damaged_saved_stacks_restore_repaired_without_error`,
      `a_hidden_root_keeps_the_tab_identity_and_a_hidden_focus_falls_back_to_a_leaf`,
      `a_stack_whose_visible_member_failed_hands_its_leaf_to_a_survivor`.)

### Docs
- [ ] `configuration.mdx` documents the keys and behaviour (English, with the
      Japanese and Simplified Chinese pages kept in heading parity), and
      `config-reference.json` lists the new keys
      (`scripts/test_config_reference_check.py` and
      `scripts/docs_translation_parity.py` pass).

### Integration (real downstream systems)
- [ ] An integration test starts a real server and client, creates a stacked
      pane, types into each, switches, and asserts that each pane kept its own
      output and that `pane list` reports both with their `stack` objects.
- [ ] An integration test restarts the server and asserts the stack is
      restored, with the same visible pane.

## Implementation notes

**Part 1 (model and lifecycle), 2026-11-19.**
`src/workspace/stack.rs` holds `PaneStack` and the stack operations on `Tab`
(`stack_existing_pane`, `show_pane`, `cycle_stack`, `unstack_pane`,
`all_pane_ids`, `is_hidden_pane`, `slot_leaf`); `TileLayout::replace_pane`
swaps a leaf's id with focus and previous focus following.
`Workspace::stack_pane_shell` spawns through the split path and folds the new
leaf into the target slot, so spawn, environment and pane numbering are
shared with splits. "Last pane" now counts pane states in
`Workspace::{remove_pane, close_pane, take_pane_for_move, close_focused}`,
`Tab::detach_pane`, the close-confirmation checks and the API's
layout-update check. Plugin cleanup, worktree-removal shutdown and the
agent-resume terminal lookup use `all_pane_ids`. Moving a stacked pane moves
that pane only, its slot passing to the next member; moving a whole slot is
built on this in part 3. Stacks are not yet saved or restored (part 2), and
nothing outside tests creates one (part 3). Verification: lint clean, nextest
4134 passed, 16 stack tests; six deliberate breakages, each caught.

**Part 2 (persistence and handoff), 2026-11-19.** `TabSnapshot.stacks`
(`StackSnapshot { panes, active }`) is omitted when empty, so stack-free
sessions serialize, and fingerprint their saved scrollback, exactly as stock
herdr does. `src/persist/stack_restore.rs` validates saved stacks before
restore (exactly one member must be a tree leaf; hidden members need a saved
pane entry; a pane belongs to one stack), gives hidden members fresh ids, and
`restore_tab` restores them through the same per-pane path as tree panes, so a
handoff import consumes their runtimes. If a visible member does not come
back, a surviving member takes its leaf before pruning. Focus resolves among
leaves only; a hidden root pane is kept. Public pane numbers and handoff
aliases cover hidden members, and deferred agent resumes include them at the
visible member's size (`Tab::with_hidden_members`). Verification: lint clean,
nextest 4141 passed; eight deliberate breakages, each caught. The live
handoff integration test comes with part 5, once stacks can be built from
outside.

**Part 3 (server, protocol, API), 2026-11-19.** `src/ui/stack_strip.rs`
owns the strip geometry (the top row of a stacked slot's content area) and
draws it into the tab surface after the panes; sizing, resize and the client
geometry share `strip_and_content`, and hidden members are resized with their
visible member. The hidden-source early exit, `pane_is_in_active_tab`, the two
inline notification checks and seen-marking treat hidden members as off
screen. Lists (`pane list`, `agent list`, the session snapshot, name targets,
sidebar details, metadata expiry) use `Tab::all_pane_ids`. `PaneInfo` gains an
optional `stack` object (the frozen method shapes are unchanged; the generated
API schema was regenerated). The `pane_stacks` capability and the
`endpoint.pane-stack.v1` / `.control.v1` controls carry strip geometry and
`new` / `show` / `cycle`; a view without stacks sends nothing. API geometry
requests on a hidden member reveal it first (`App::reveal_geometry_targets`).
Verification: lint clean, nextest 4156 passed; 15 deliberate breakages, all
caught once two test gaps were closed (active-tab resize, workspace-filtered
`pane list`). Known gap: a hidden pane's respawn size goes through its slot
(`new_pane_terminal_size`), untested.

## Test strategy (for review before tests are written)

- **Characterization first:** before moving code, name the existing tests
  that protect close-tab, save and restore, handoff, directional focus and
  agent lists, and add any that are missing.
- **Unit (layout):** `TileLayout::replace_pane` keeps ratios and paths;
  strip-row geometry; slots too short for a strip.
- **Unit (model):** stack create, switch, close, exit, last member left, and
  split, swap, move and zoom of a stacked slot; the invariant checks with
  hidden panes, including the adversarial identity fixtures.
- **Server tests** with test runtimes, as for the dock: the strip drawn into
  the tab surface, the visible pane's rect below the strip, hidden-pane
  sizing, hidden output taking the early exit, and agent state of hidden
  panes reaching agent lists and notifications.
- **API tests:** `pane list` `stack` objects; `pane read` and `pane run` on a
  hidden pane.
- **Client-shell tests:** strip clicks, keys and wheel turned into controls.
- **Compatibility:** stock v0.9.5 against patched builds in both directions,
  and a stock restore of a session with stacks.
- **Integration:** the two real server-plus-client tests above.
- **Mock boundaries:** none for terminals, the protocol or session files.
- **Regression guard:** each behavioural test is checked to fail with its
  behaviour removed.

## Risks & Assumptions

- **Invariant change (high):** this is the first fork patch to change a core
  workspace invariant, not add beside it. A missed enumeration site can hide
  a pane from close-tab, lose it on handoff, or resize it to zero. Phase 0 is
  the mitigation and the stop point.
- **Rebase cost (high):** it touches workspace, layout, persistence and the
  renderer. Each upstream release will cost more to port than the spaces bar
  or the dock. It is a separate series so it can be dropped.
- **Rollback:** the side-field design means rollback loses the hidden panes
  (their processes end) but keeps the session. Same pattern as the dock. A
  live handoff to stock herdr fails safely while any stack exists (stock's
  import rejects unused runtimes), so rollback to stock is a cold restart:
  `herdr server stop`, swap the binary, start. Decided 2026-11-19.
- **Stock clients** see the strip but cannot click it, and their keymaps lack
  the new actions. Accepted.
- **Assumption:** the panel use case is served by creating stacked panes by
  hand, with `prefix+t` then starting the tool. v1 has no CLI to build a
  stack, so the panel script cannot build one itself (see below). For the
  restore test, herdr itself restores the stack structure; herdr-resurrect
  only re-launches programs, which works through `pane list` and `pane run`.

## Deliberately not supported (v1)

- Stacking an existing neighbouring pane into a slot, or unstacking a pane
  back into its own split.
- A CLI or API to build stacks. As a result, scripts such as the panel script
  cannot set up a stack. This is the most likely follow-up.
- Dragging stacked panes between slots, or reordering them within a strip.
- Names for stacked panes separate from pane names (the strip uses the pane
  label).
- Strip placement other than the top row.
- Pinning a slot to a fixed size (spec 003).

## E2E Test Plan

Environment: the development machine (Linux x86_64). Steps 1–5 run on a
throwaway `--session` driven through a private `tmux -L` harness, which is
test tooling, not part of the user's setup. Steps 6–7 run live after the
user's desk test.

1. In a split layout, `prefix+t` in the right pane → a strip appears in that
   slot with two entries, the new shell is visible and focused, and the left
   pane's rect is unchanged. *(Create; layout untouched.)*
2. Run `yazi` in the first stacked pane and `lazygit` in the second, then
   switch with `prefix+.`, `prefix+,`, a strip click and the wheel → each tool
   redraws intact; no reflow. *(Switch; hidden sizing.)*
3. Start an agent in a hidden pane and let it block → its glyph shows in the
   strip, and `herdr agent list` shows it. *(Hidden agents.)*
4. `exit` in the visible pane → its neighbour shows; with one pane left, the
   strip disappears. *(Close; collapse.)*
5. Restart the server → the stack, its order and visible pane are restored;
   attach a stock v0.9.5 client → the strip renders. *(Persistence;
   compatibility.)*
6. Live, with herdr-resurrect ported to the fork: hayami in the dock, yazi and
   lazygit in one stack. Restart the server and run the resurrect restore →
   the stack comes back with both tools running and the dock with hayami.
   *(The user's acceptance test.)*
7. Live: use that layout for a day in place of the yazi/lazygit splits.
   *(Real use.)*

### Prod visibility

Not applicable. This is a personal workstation tool with no production
deployment. Observability is herdr's server log; stack creation, switching,
closing and restore repairs are logged with pane ids.

## Alternatives Considered

- A new layout node type for stacks: rejected by the user. Stock herdr could
  not load such a session, so rollback would lose the whole layout.
- Keeping hidden panes in a store separate from `Tab::panes`, as the dock
  does: rejected, because hidden agents would vanish from agent lists and
  notifications.
- The strip in the pane border title: rejected by the user, since it would
  exist only while borders are drawn.
- A client-drawn strip, like the dock's rule: considered; rejected because
  the strip sits inside the tab surface, which the server draws with its
  borders, and stock clients would not show it.
- tmux or another multiplexer inside a pane: rejected by the user, because
  herdr is the only session layer.
- Calling them "slot tabs": renamed to pane stacks after review, because
  "tab" already means a workspace tab throughout herdr's code and config.
