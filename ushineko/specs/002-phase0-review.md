# Independent review of the Phase 0 enumeration audit (spec 002, pane stacks)

Scope: `ushineko/pane-stacks` at `b3fb12a7` (audit line numbers at `e409d67d`; the
files cited here were re-read at `b3fb12a7` and the numbers match). Read-only.

Method: independent searches for every pane-to-tab lookup (`find_tab_index_for_pane`,
58 non-test callers), every `layout.focused()` reader, every `root_pane` reader,
`terminal_id_for_pane`, public-pane-number maps, runtime-keyed loops
(`terminal_runtimes.iter/values`), the client-side consumers of `snapshot.panes` and
`surface.panes` (`src/client/shell/**`), the server projections (`client_shell.rs`,
`render_stream.rs`, `retained_surface.rs`, `headless/dock.rs`), agent-resume,
worktree, plugin and dock code. Each hit was read in context and compared with the
audit's row for that function, or noted as absent.

Overall: the audit's enumeration of `pane_ids()` / `pane_count()` / `tab.panes` sites
is complete and its three themes (A, D, E) are verified. What it under-covers is the
layer above the enumerators: API handlers that reach a hidden pane by id through
`find_tab_index_for_pane` (which succeeds) and then hand it to a layout mutator (which
silently refuses), the dock's dirty-source gate, inline copies of the active-tab
check in the notification path, and what the client can know about hidden panes given
the frozen `ClientShellSnapshot`.

---

## 1. Missed sites

### M1. Inline active-tab checks in the notification path (major)

`src/server/headless/notifications.rs:366-373` and `:460-467`. Both compute
`is_active_tab` inline as `ws.find_tab_index_for_pane(pane_id) == ws.active_tab_index()`,
not through `AppState::pane_is_in_active_tab`. The audit's E2 / row 19 fix changes
`pane_is_in_active_tab` (`src/app/actions.rs:375-385`); these two copies are not
reached by that fix, so a hidden member of the active tab still suppresses its sound
and semantic notification there (`active_tab_suppresses_notifications` → no sound,
toast kind downgraded). The spec decided hidden = not visible for notifications.

Fix: route both through `pane_is_in_active_tab` (or the new stack-aware helper), and
extend the test that pins hidden-pane notifications to the headless forwarding path,
not only `apply_pane_state_change`. `notifications.rs:122` and `headless.rs:3191`
already call `pane_is_in_active_tab` and are covered.

### M2. Dock dirty gate counts hidden dock members as visible (major, hot path)

`src/server/headless/dock.rs:50` `dock_sources_dirty` = `sources.any(|id| dock.contains(id))`;
`Dock::contains` (`src/app/dock.rs:194`, audit row 59 "OK") tests `dock.panes`, all
members. Called at `src/server/headless.rs:560`; when true, `:603` runs
`stream_dock_surfaces` → `render_dock_surface` (draw every visible dock pane's screen,
JSON-encode, string-compare per opted-in client) even though the dirty source is
hidden and nothing it drew changes. This is exactly the AGENTS.md "hidden output must
not trigger presentation work" case, on the dock side, and the audit's theme E does
not mention it.

Fix: `dock_sources_dirty` must test leaf membership (`dock.layout.pane_ids()` or the
stack-aware `is_visible`), the same change as row 78. Add a dock test beside
`agents_in_dock_panes_raise_no_list_entries_or_notifications`.

Related, pre-existing (spec 001, out of scope but relevant to the bench AC):
`pty_source_visible_to_render_targets` (`render.rs:374-376`) resolves the terminal via
`App::find_pane`, which only knows workspace panes, so a dock pane source has
`terminal_id == None` and is treated as visible. While any dock pane is dirty,
`hidden_only` can never be true. Hidden stack members in the dock therefore also defeat
the workspace-side early exit for that frame.

### M3. Client-side consumers of `snapshot.panes` once hidden panes are listed (major, decision)

Row 22's fix adds hidden panes to `SessionSnapshot.panes`, which `client_shell.rs:123-150`
copies into `ClientShellPane` (`src/protocol/wire.rs:1058-1067`). That struct has no
visibility field, and the spec forbids changing `ClientShellSnapshot`. So a client
cannot tell a hidden pane from a visible one in `snapshot.panes`. The audit tabulates no
client sites and records this as "unverified". Sites that change behaviour:

- `src/client/shell/actions.rs:1100-1125` (`endpoint_method_for_action`,
  `CyclePaneNext/Previous`): builds the cycle list from `snapshot.panes` filtered by the
  focused tab, then sends `PaneFocus`. With hidden panes listed, cycling walks into
  hidden members and, after E1, reveals them (a stack switch as a side effect of
  "next pane"). The other cycle path, `input.rs:1010-1040` `cycle_pane`, uses
  `surface.panes` and is already visible-only; the two paths will disagree.
- `src/client/shell/aggregate_navigation.rs:325`: Go To lists hidden panes under their
  tab (desired by the spec), with no marker that they are hidden.
- `src/client/shell/state.rs:1507-1540`, `word_selection.rs:179`: "pane still exists"
  checks for selection, word-gesture and copy mode. A pane that becomes hidden while a
  selection or copy mode is on it still "exists"; the surface lookups at `state.rs:1782-1817`
  then find no geometry for it. Today those states are cleared when the pane leaves the
  snapshot; with stacks they are cleared only by the focus change. Needs a test.
- `src/client/shell/overlay_input.rs:1024-1040` (confirm-close "would close the
  workspace") and `:1186` (worktree-group pane count): counting hidden panes is the
  correct answer here, so these improve.

Decision needed before implementation: (a) relax the rule and add
`#[serde(default)] visible: bool` to `ClientShellPane` (the struct derives serde only,
not `bincode::Decode`, so an optional field is wire-compatible; the constraint is the
spec's own rule, not the protocol); or (b) have patched clients intersect
`snapshot.panes` with `surface.panes` (viewed tab only) or with the
`endpoint.pane-stack.v1` control. Stock clients get (b)'s behaviour regardless. See §4.

### M4. Geometry mutators reached with a hidden pane id: split, swap, move (major for move; minor otherwise)

The audit's rows 30/31 cover `pane edges` / `pane neighbor`; three more API handlers
accept a hidden id (because `find_tab_index_for_pane` succeeds) and then hit a layout
mutator that silently refuses:

- **Split.** `src/app/api/panes.rs:34-115` `handle_pane_split` → `Workspace::split_pane`
  → `Tab::split_pane_with_runtime` (`src/workspace/tab.rs:329-337`) →
  `TileLayout::split_pane` (`src/layout.rs:182-192`, not in the audit's enumerator
  list) returns `None` → `Err(NotFound, "split target pane is not in the layout")` →
  API `pane_split_failed`. Also `new_pane_size(Split { target: hidden })`
  (`src/ui/panes.rs:117-125`) gets `None` from `panes_after_split` and sizes the new
  shell to the whole area. Spec: reveal first, then act. Minor.
- **Swap by id.** `src/app/api/panes.rs:873-893`: `reason` is `None`, `swap_panes`
  returns `false`, `focus_pane(hidden)` no-ops, and the response is
  `changed: false, reason: None` — a success with no explanation. Row 6 ("OK") is right
  at the layout level and incomplete at the API level. Minor.
- **Move with a hidden `target_pane_id`.** `src/app/api/panes.rs:1083-1097` validates
  the target only by tab membership; `:1138-1143` then **takes the source pane out of
  its tab**, `:1197-1215` `insert_moved_pane_into_tab` fails (`insert_pane_near`,
  `layout.rs:197-222`, cannot find a hidden target), and `recover_failed_pane_move`
  (`:1369-1377`) does not put the pane back: it creates a **new tab** in the source
  workspace holding the pane. So `pane move --target <hidden id>` ejects the source
  pane into a fresh tab and reports `pane_move_failed`. The recovery path is
  pre-existing; a hidden target makes it reachable with a valid id from `pane list`.
  **Major.** Fix: reveal the target (or reject a hidden target) before `take_pane_for_move`.
- **Dock split.** `Dock::split` (`src/app/dock.rs:498-510`, row 66 "OK") has the same
  shape: hidden target → `split_pane` `None` → silently `false`. Harmless, but a
  decision site like the above.

### M5. `TileLayout::replace_pane` must carry focus history (minor, fix-description gap)

The audit's fix for A and for switching is a new `replace_pane`. `close_focused`
(`src/layout.rs:224-252`) shows what the leaf-removal path maintains: `focus`,
`prev_focus` (dropped when it names the removed pane) and the "next, else previous"
rule. `replace_pane(old, new)` needs the same: `focus == old → new`,
`prev_focus == old → None` (or `new`), otherwise `prev_focus` naming a now-hidden pane
is tolerated by `close_focused`'s `ids.contains(&prev)` guard but will surprise
`LastPane`. Not listed in the audit.

### M6. Restore re-roots a tab whose `root_pane` is hidden (minor)

`src/persist/snapshot.rs:440` saves `root_pane` as whatever `tab.root_pane` is, which
may be a hidden member (stacking the original root hides it; `promoted_root_if_needed`
only runs on close). `src/persist/restore.rs:783-787` resolves it through `id_map`,
which `restore_node_remapped` builds from the tree only, so a hidden root falls back to
the first tree pane without a log line. Workspace identity cwd follows
`tabs[0].root_pane` (`src/workspace.rs:1018, 1042`), so a workspace's label can change
across a restart. Fix: the stack-restore step must insert hidden members into `id_map`
and `surviving` before `resolve_restored_pane` runs (rows 55/56 do not say so).

### M7. Row 57 is a prerequisite, not a peer (dependency, not a new site)

`restore_workspace` (`restore.rs:388-404`) assigns public numbers for tree ids only.
`App::public_pane_id` then returns `None` for hidden members, and `pane_info`,
`agent_info`, `terminal_target_candidate` and `forward_semantic_agent_transition`
(`notifications.rs:62`) all `?` on it. So after a restore, rows 23/24/25/38 stay broken
until row 57 is fixed, even after their own fixes. Worth ordering in the plan.

### Checked and found OK (one line each)

- `root_pane` readers (`workspace.rs:607-611, 1018, 1042`, `layouts.rs:505-512`,
  `snapshot.rs:342`, `aggregate.rs`): all tolerate a hidden root; only M6 matters.
- `read_checked_pane_link` (`src/app/api/plugins/mod.rs:269`) refuses a hidden pane with
  `stale_target`: correct.
- `restore_overlay_after_exit` (`src/app/api.rs:531-533`): audit row 35 is right.
- `retained_surface.rs:311-330`: a source absent from every recipient surface is
  `continue`d, not a fallback, so a mixed visible+hidden dirty set does not force a
  full render.
- `theme_sync.rs:50-56`, `api.rs:433`: runtime-keyed loops include hidden runtimes.
- `kitty_graphics/surface.rs:594` iterates `pane_infos` (visible): correct.
- `custom_commands.rs:144`, `plugins/context.rs:332`, `ids.rs:48`,
  `terminal_targets.rs:162-178`: lookups, correct for hidden panes.
- `shell_visible_pane_wants_sgr_pixels`, `sync_immediate_pty_sources`,
  `shell_client_views_pane`, `pane_visible_on_active_surface`: leaf-based, correct.
- `close_focused_pane_would_close_workspace` (`actions.rs:928`) and
  `Workspace::close_focused` (`workspace.rs:850`) are `#[cfg(test)]`; not production sites.

---

## 2. Wrong or incomplete verdicts (20 "OK" rows spot-checked)

| Row | Function | Verdict after check |
|---|---|---|
| 3, 4 | `TileLayout::close_focused` / `close_pane` | OK. Internal guards; correct once callers use `replace_pane`. |
| 5 | `TileLayout::focus_pane` | OK. Silent no-op is what keeps focus off hidden panes. |
| 6 | `TileLayout::swap_panes` | **Incomplete.** Layout refuses correctly; the API caller returns `changed:false, reason:None` (M4). |
| 12 | `terminal_ids_for_workspace` | OK, confirmed (`tab.panes.values()`). |
| 14 | `terminal_ids_for_tab` | OK, confirmed. |
| 16 | `remove_unattached_terminal_ids` | OK, confirmed. |
| 17 | `apply_pane_zoom` | **Conditional.** Today with a hidden id in a multi-slot tab: `focus_pane_in_workspace` switches tab, `focus_pane` no-ops, returns `true`; the tab is then zoomed on the *current* leaf and the API reports `focus_changed: true`. OK only after E1. |
| 19 | `apply_pane_state_change` | OK as a lookup; E2 covers `pane_is_in_active_tab` but not the inline copies (M1). |
| 26 | `tab_info` | OK. `pane_count` includes hidden members; consistent with the client's own counts (`overlay_input.rs:1186`). |
| 28 | `handle_layout_apply` | OK; `layout apply` cannot build stacks in v1. |
| 36 | `App::try_new` restored cwds | OK, confirmed (`snapshot.panes` has hidden members). |
| 39 | `Workspace::aggregate_state` | OK, confirmed, and load-bearing: a hidden blocked agent does raise the workspace attention state. |
| 41 | `Workspace::close_tab` | OK, confirmed. |
| 44 | `find_tab_index_for_pane` etc. | OK. |
| 48 | `promoted_root_if_needed` | OK. |
| 49 | `insert_existing_pane` | **Incomplete.** "Target must be a leaf" is stated; the consequence when it is not (pane ejected to a new tab, M4) is not. |
| 56 | `restore_tab` surviving / pruning | **Incomplete.** See M6 (`root_pane`). |
| 59 | `Dock::contains` | **Wrong** for its `dock_sources_dirty` use (M2). OK for the other callers. |
| 66 | `Dock::split` | OK but a decision site (M4). |
| 74 | `perform_live_handoff` export | OK, confirmed: `pane_by_terminal` from all `tab.panes`. Note `disconnect_all_clients_for_handoff` (`lifecycle.rs:58`) runs before the import attempt, so a failed handoff to stock drops every client; they reconnect after `wait_then_restore_public_sockets_after_failed_handoff`. Safe, not transparent. |
| 76, 77, 79, 80, 81–83, 86, 89 | render/view/notification lookups and projections | OK, confirmed. Row 89 should add that `ClientShellPane` cannot carry visibility (M3). |

Tally: 1 wrong (59), 4 incomplete (6, 17, 49, 56), 15 confirmed.

---

## 3. The critical themes

### A. Last-pane checks via `layout.pane_count()` — verified, with two refinements

Confirmed at `src/workspace.rs:869-893` (`remove_pane`), `:895-915`
(`take_pane_for_move`), `:1111-1137` (`close_pane`), `src/workspace/tab.rs:460-479`
and `:501-517`, `src/app/actions.rs:938-943`, `src/app/api/panes.rs:2199-2213`.

- One-tab workspace: `remove_pane`/`close_pane` return `true`; `handle_pane_died`
  (`actions.rs:2081-2089`) and the API close (`api/panes.rs:1995-2000`) then detach
  **all** workspace terminals (`terminal_ids_for_workspace`, all panes). So in that
  case the surviving members are **killed with the workspace**, not leaked. The audit's
  "whole workspace closed" is right; "runtimes keep running" applies only to the
  multi-tab case (`actions.rs:2118-2119`, `tab.remove` with one `unregister_pane`).
- Multi-tab case: besides the runtime leak, the removed tab's other members keep their
  `public_pane_numbers` entries, so `Workspace::assert_invariants_for_test`
  (`workspace.rs:1387-1392`) fails: the fixture extension will catch this.
- `Tab::take_pane_for_move` on a hidden member of a multi-slot tab works by accident
  (`layout.close_pane` is a silent no-op); make that explicit rather than rely on it.

Not worse than stated.

### D. Strict handoff import — verified; two mechanics to add

`restore.rs:189-227` (`restore_with_imports_strict`: unconsumed imports → `Err`),
`:508-516, :581-590` (consumption keyed on tree ids), `lifecycle.rs:48-55, 81-103`
(export of every `tab.panes` runtime). Confirmed; every handoff fails while a stack
exists, until restore learns stacks.

- `handoff_pane_aliases` (`restore.rs:122-140`) zips old **tree** ids with new
  `layout.pane_ids()` positionally. Hidden members must be aliased from the stack
  restore's own old→new map, never appended to this zip, or the positional pairing
  misaligns. Row 58's "also map hidden members" should say how.
- Clients are disconnected before the import attempt (row 74 note), so a failed
  handoff is a visible blip even though the old server survives.
- M6 (`root_pane`) and M7 (public numbers) apply to the handoff import too.

### E. `any_shell_surface_contains_pane` — verified; one more site, not worse

`src/server/headless/render.rs:441` tests `tab.panes.contains_key`, reached from
`:374-376` and `:379-393`, gated at `src/server/headless.rs:588-592`. Confirmed.
The cost when a hidden-only burst is miscounted as visible: the retained path runs
(`retained_surface.rs:230-330`: target collection, `has_synchronized_pane` runtime
lookups for every visible pane per recipient, per-source lookup, then `continue` on the
missing pane) and `record_render_attempt(now, true)`; not a full render. So "triggers
presentation work" is accurate and not understated.

Additional E-class site the audit missed: `dock_sources_dirty` (M2).

---

## 4. Containment

No site needs a change to the model in the spec (hidden members in `Tab::panes`, visible
member as the leaf, `stacks` beside the tree). Every fix above is a local substitution,
a reveal-before-act call, or a helper. Two items are design decisions rather than
fixes, and should be settled before code moves:

1. **Client knowledge of visibility (M3).** `ClientShellPane` cannot say "hidden" and the
   spec freezes `ClientShellSnapshot`. Either relax that rule for one optional serde
   field (wire-compatible; the struct is not bincode-frozen), or accept that clients
   derive visibility from `surface.panes` (viewed tab only) and the pane-stack control
   (patched clients only). Stock clients will cycle and Go To into hidden panes and
   reveal them via `PaneFocus`; decide whether that is acceptable.
2. **Geometry API on a hidden pane (M4).** The spec's "reveal first" rule must be
   applied in the API layer (before `take_pane_for_move`, before `split_pane`, before
   `swap_panes`), not only in the layout. The move case is the one with a destructive
   failure mode today.

One item to verify during implementation rather than a containment risk: a stack switch
replaces a leaf id in place without changing geometry. The retained-surface planner
patches by `pane_id` against each client's last `PaneSurfaceFrame`
(`retained_surface.rs:286-299` baseline checks on `projection_revision`). The switch
must bump the client's projection revision (or mark `requires_recompute`) so the next
frame is a full render; otherwise the planner has no row for the new pane and silently
sends nothing for that slot. The existing `request_recompute` on geometry change
(`client_views.rs:521-523`) should cover it if the switch goes through the same path.
