# 000 — herdr horizontal spaces bar

> Personal fork patch. Not part of herdr; not affiliated with the herdr project.

## Status: COMPLETE

A retro-spec: the feature was built first (on herdr 0.7.4) and this records
the intended behaviour, the decisions behind it, and the invariants found in
testing, so later ports do not have to re-derive them. It has since been ported
to 0.8.2, 0.9.1 and 0.9.5.

Branch: `ushineko/spaces-bar`.

## What it does

`ui.sidebar_layout = "horizontal"` moves spaces out of the left rail into a bar
above the tab bar:

```
 +  <  1 ◐ monitor   2 ○ backend  ×1 ◐2   3 · docs       ✓  >      menu
─────────────────────────────────────────────────────────────────────────
 monitor     dmesg    +
```

(Drawn with `ui.status_indicators = "symbols"`; since 0.8.0 the default `dots`
style distinguishes agent states by colour only.
The `<` / `>` buttons appear only when the chips overflow the strip.)

Each chip carries the space number, rolled-up state icon, name, and a folded
agent summary (one glyph per state class, with a count when more than one agent
shares that state). Clicking a chip switches space. Clicking the status glyphs
focuses the space's single agent, or opens a picker listing every agent in that
space when there are several. The strip scrolls with the wheel or the `<` / `>`
buttons at each end, and a horizontal
rule separates it from the tab bar below.

`prefix+shift+b` switches placement live, without editing the config.

Vertical placement remains the default and is untouched.

---

## Design (retro-spec)

No spec was written before implementation. This section records the intent,
behavior contract, and the reasoning behind the non-obvious choices, so a future
rebase or a port to another OS does not have to re-derive them.

### Problem

The vertical rail costs 18–36 columns permanently. It buys two things: switching
between spaces, and triaging agents across all of them. On a wide monitor the
first is worth far less than the columns it consumes. The goal was to spend one
or two *rows* instead of tens of columns, keeping space switching and enough
agent awareness to know where attention is needed.

### Behavior contract

What the layout guarantees, in the form acceptance criteria would have taken:

| # | Behavior |
| --- | --- |
| 1 | `ui.sidebar_layout = "horizontal"` moves spaces to a top bar; `"vertical"` (default) leaves all existing behavior untouched. |
| 2 | The setting applies on `herdr server reload-config` without restarting panes. |
| 3 | The bar occupies exactly 2 rows: one chip row, one separator rule. |
| 4 | Each chip shows: space number (≤ 9), rolled-up state icon, space name (truncated at 18 columns), and a folded rollup of the agents in the current agent view. |
| 4a | Chip and rollup glyphs honor `ui.status_indicators`, so `"symbols"` gives the bar the same distinct shapes it gives the rail. (v0.8.2 onward; the glyphs are static — [upstream removed the spinner in 0.8.0](#agent-status-is-static-from-080-on).) |
| 5 | The rollup shows at most 3 state classes, ordered by attention priority, each with a count when > 1. |
| 6 | A collapsed worktree group's parent chip rolls up its members' agents, so folding a group cannot hide a blocked agent. |
| 7 | Clicking a chip switches to that space (on mouse *up*, matching the rail). |
| 8 | Clicking a chip's rollup focuses the agent when the space has exactly one; opens a picker when it has several. |
| 9 | The picker lists the same agents as the rollup — icon, agent, tab/pane, status — most urgent first; choosing one focuses that pane. An active `agent.view.set` filter applies here as it does to the rail and mobile (v0.8.2 and the retired master branch; v0.7.4 has no agent views). |
| 10 | The wheel scrolls the strip; the focused space is always revealed **fully**, never clipped. |
| 10a | When the chips overflow the strip, `<` and `>` buttons at each end scroll it by one chip per click, on mouse *down*. They move the same offset the wheel does. |
| 10b | The buttons are reserved out of the strip only when the chips genuinely overflow **and** mouse chrome is on. A bar with room to spare spends no columns on them. |
| 10c | Below `MIN_SCROLLABLE_STRIP_WIDTH` (2 buttons + 3 columns) the buttons would crowd out the chips they scroll, so the bar keeps the chips and falls back to the `…` overflow hint — which is also what a bar with mouse chrome off shows. |
| 10d | An arrow is drawn in `overlay1` when it can still scroll and `overlay0` when it cannot, matching the tab bar's buttons. |
| 10e | A manual scroll (wheel or button) **owns** the offset: the focused-chip reveal in row 10 stops pulling the strip until the focused space actually changes. Without this the reveal recomputes the offset every frame and erases the scroll on the next render. |
| 11 | The strip never scrolls past the tightest offset that fully reveals the last chip. Up to one chip's width of space can remain at the right edge at maximum scroll — chips are left-aligned, as in the tab bar. |
| 12 | The bar has no collapsed form: `toggle_sidebar` (`prefix+b`) does nothing while it is shown and changes no hidden state. It still collapses the vertical rail. |
| 13 | Terminals at or below `ui.mobile_width_threshold` keep the mobile layout regardless of this setting. |
| 14 | `keys.toggle_sidebar_layout` (default `prefix+shift+b`) flips placement live. |
| 15 | A layout toggled by keybinding is runtime-owned and survives `reload-config`; editing `ui.sidebar_layout` and reloading still works when it has not been toggled. |
| 16 | A layout toggled by keybinding is written to the session snapshot and reapplied on import, so it survives a server restart **and** a live handoff. A placement still owned by config is *not* persisted, so `ui.sidebar_layout` keeps working after a session is saved. |

### Design decisions

| Decision | Reasoning |
| --- | --- |
| A **separate layout**, not a mode of the existing sidebar | The rail's geometry is row-based throughout (card areas, scroll metrics, drag targets). Reusing it would mean threading an orientation flag through all of it. A parallel `compute_horizontal_view` keeps vertical behavior provably unchanged. |
| Agent panel **folded into counts**, not listed | A per-agent list needs vertical space, which is the one thing the layout is trying to reclaim. Counts preserve awareness; the picker restores per-agent access on demand. |
| Rollup capped at **3 state classes**, sorted by attention | Chips must stay narrow enough that several fit. Attention-priority ordering means the truncated tail is always the least interesting. |
| Reveal requires the chip be **fully** drawn | A chip clipped at the right edge counts as not revealed. Without this the strip stops scrolling while the focused space is cut in half. |
| One row reserved for a **separator rule** | Direct analogue of the column the vertical rail reserves for `│`. Without it a space chip and a same-named tab below run together. |
| Rule drawn in **`overlay0`**, not `surface_dim` | `surface_dim` is a *fill* colour: it equals `panel_bg` exactly in 8 of 18 built-in themes, so the rule vanished. `overlay0` is what unfocused pane borders use and has contrast in every theme. Guarded by a test over all themes. |
| Picker built on the **context menu**, not a new popup | Hover, keyboard navigation, click-away and Escape come free, and it follows upstream's "reuse existing UI patterns" rule (`AGENTS.md`). Menu items are `&'static str`, so the new kind carries its own rows rather than changing the shared type — every other menu and all 17 pre-existing construction sites stay untouched. |
| Picker rows are a **snapshot**; focus resolves the tab **live** | An agent can exit or move tabs while the menu is open. Focus goes through `focus_pane_in_workspace`, which finds the tab from the pane id, so no stale index is ever used. The debug invariant checker deliberately does not assert those panes are alive, or a pane exiting under an open menu would panic debug builds. |
| Toggle binding defaults to **`prefix+shift+b`** | The obvious mnemonics were taken: `prefix+h` is `focus_pane_left`, `prefix+minus` is `split_horizontal`. Two actions cannot share a trigger — the second registration is disabled and reported — so either would have shipped a broken default. `prefix+shift+b` is free and reads as the shift variant of `toggle_sidebar`, matching how `prefix+w`/`prefix+shift+w` and `prefix+n`/`prefix+shift+n` pair up. A test asserts the shipped keymap produces no diagnostics. |
| Placement is **persisted in the session snapshot** | `toggle_sidebar_layout` marks the session dirty, so the snapshot has to carry the layout or the toggle is silently lost — on restart, and on every live handoff, which transfers the same `SessionSnapshot`. Stored as an `Option`, so pre-existing snapshots restore as `None` and fall back to config; no snapshot version bump. Restored values get `SidebarLayoutSource::Persisted` so a later reload does not overwrite them. |
| Hit tests read **cached** chip geometry | `compute_view` stores the laid-out `SpacesBarView` in `ViewState`. Rebuilding geometry in the input path needed a `TerminalRuntimeRegistry`, and passing an empty one produced different chip widths than were rendered for workspaces auto-labelled from pane cwd — clicks could land on the wrong space. |
| A runtime toggle **outranks** a later `reload-config` | Mirrors `SidebarWidthSource`: a manually dragged width is not reset by a reload, so a manually toggled layout is not either. Without it, reloading after an unrelated config edit would snap the layout back. |
| `toggle_sidebar` made **fully inert** in horizontal, and the collapsed bar removed | Flipping a hidden flag that has no visible effect strands state. And since the bar is what would be collapsed, a collapsed bar could never be reopened — `prefix+b` no longer reaches it. |
| No new server API | Sidebar placement is TUI presentation state per `AGENTS.md`'s runtime/client boundary rule; nothing here is exposed over the socket API. |

### Deliberately not supported

These need vertical room or row geometry, and the bar drops them rather than
faking them. Each is made inert in horizontal placement, not left to misfire:

- Separate agent panel section, and its sort toggle.
- Sidebar width drag and section-split drag.
- Collapse entirely — the bar has no collapsed form, and `toggle_sidebar` is inert while it is shown.
- Drag-to-reorder spaces.
- Group collapse/expand chevron — folding stays on the context menu.

### Alternatives considered

- **Right-click menu entries for agents** — cheapest, but right-click is
  undiscoverable and it mixes navigation into a menu that is otherwise about
  managing the space.
- **Desktop reuse of the mobile agents-first switcher** — already exists and
  scales past a handful of spaces; heavier than needed for "switch to the other
  agent in this space". Still the best answer for cross-space triage if that
  becomes a need.
- **Expandable agents row under the bar** — good within one space, costs a row,
  does not help across spaces.
- **Bar and rail coexisting via `toggle_sidebar`** — honest fallback, since the
  rail genuinely is better for triaging many agents, but it spends the screen
  real estate the bar just won back.

### Code map (0.9.x)

From 0.9.0, herdr renders the shell in the client (`src/client/shell/`), so the
bar is client-side presentation: no server state and no protocol change.

| Path | Role |
| --- | --- |
| `src/client/shell/spaces_bar.rs` | The whole bar: chip content and widths, scroll and scroll buttons, hit geometry, status rollup, the agent picker rows, rendering. |
| `src/client/shell/config.rs` | Layout: the 2-row bar above the tab bar when placement is horizontal. |
| `src/client/shell/state.rs`, `preferences.rs` | Per-client placement, saved with the client chrome preferences. |
| `src/client/shell/actions.rs`, `mouse.rs`, `context_menu.rs` | `toggle_sidebar_layout`, chip and rollup clicks, the agent picker as a context menu. |
| `src/config/model.rs`, `src/config/keybinds.rs`, `src/input/keybindings.rs` | `ui.sidebar_layout` and `keys.toggle_sidebar_layout`. |
| `src/client/shell/tests/spaces_bar.rs` | Behaviour tests. |

Entry points: `compute_spaces_bar_view()`, `render_spaces_bar()`,
`status_agents()`, `scroll_spaces_bar()`.

Some sections below were written against the 0.7.4/0.8.2 implementation, in
which the bar lived in `src/ui/sidebar/horizontal.rs` and `AppState`. The
behaviour contract and invariants carried over unchanged.

### Bugs found in testing, and their invariants

Worth knowing because a rebase could reintroduce them:

- **Focus ratcheted the strip sideways.** `ensure_workspace_visible()` measures
  visibility with `compute_workspace_card_areas()`, whose body rect reserves two
  rows for a section header — on a one-row bar it always returns zero cards,
  concluded "not visible", and bumped the scroll once per call. Selecting a tab
  calls it. Now skipped entirely in horizontal placement.
- **Clipped chips counted as revealed**, so scrolling stopped early.
- **The separator was invisible** in half the built-in themes (fill vs line colour).
- **Picker clicks did nothing** until the App-level dispatcher was patched too.

**The focus reveal erased every manual scroll.** `compute_spaces_bar_view` ran
`scroll_revealing` on *every* frame, recomputing the offset from the focused
chip. With the focused space at the left of the strip, `scroll_revealing` takes
its `current > target` branch and collapses the offset straight back to 0, so a
scroll was undone on the next render. Found 2026-09-04 when the new scroll
buttons visibly did nothing — but **the wheel had the same defect from the
start**; it only looked like it worked when the focused space happened to sit to
the right of the offset.

The fix is the tab bar's existing pattern, `tab_scroll_follow_active`: a flag
cleared when the user takes the offset and set again when focus moves. The
spaces-bar equivalent is `spaces_bar_follow_focus`, cleared in
`scroll_spaces_bar`. Re-arming compares the focus entry against
`spaces_bar_revealed_focus` in `compute_horizontal_view` rather than setting the
flag at each of the nine `active`/`selected` write sites — detecting the change
where the bar already reads focus cannot be forgotten when a tenth is added.

> The regression test has to call `compute_view` **again** after the click. The
> original test asserted `workspace_scroll > 0` immediately after `handle_mouse`
> and passed against the broken build, because the offset was only undone on the
> next frame. Any test of scroll state that does not re-render proves nothing.

A later `codex review` pass against this contract found four more, all in the
two commits written just before it — a good argument for running an independent
pass before calling a patch set done:

- **Live handoff dropped the placement.** Capture was wired, import was not, so
  a toggle survived a restart but reverted on every hotswap.
- **A collapsed rail captured the bar's clicks.** `sidebar_collapsed` is kept
  while the bar is shown so the rail can be restored; the collapsed-rail branch
  ran first and ate chip, rollup and menu clicks, with nothing able to clear it.
- **Hit tests could disagree with what was drawn** — they rebuilt geometry with
  an empty terminal registry, so chip widths differed for workspaces labelled
  from pane cwd.
- **Persisting the placement froze config out.** Every save wrote the layout and
  every saved value restored as `Persisted`, so `ui.sidebar_layout` stopped
  applying after any saved session, even for users who never toggled it.

### Tests

`src/ui/sidebar/horizontal.rs` — chip layout without overlap, scroll reveals the
focused chip fully, clipped-chip handling, stale-offset clamp, icon padding, chip
hit testing, chrome rendering, separator rule present, separator contrast across
all 18 built-in themes, active-chip accent.

`src/ui/sidebar/horizontal.rs` (scroll buttons) — buttons appear only on
overflow, sit outside the chips they scroll, are absent without mouse chrome,
are dropped when the strip is too narrow for them, render as arrows, and dim the
exhausted side.

`src/app/input/sidebar.rs` — scroll-arrow clicks move the strip both ways and do
not switch space, bar geometry and terminal area, chip click focuses
space, focus does not ratchet the strip, single agent focuses directly, several
agents open an ordered picker, picker row focuses the pane, wheel scrolls chips,
no divider drags, menu opens below the bar, layout toggle flips placement and
claims it from config, `toggle_sidebar` inert in horizontal but still collapsing
the rail, and a toggled layout reaching the session snapshot.

`src/config/model.rs` — `ui.sidebar_layout` parsing, and
`default_keymap_has_no_conflicting_bindings`, which is what stops a future
default binding from silently disabling another action.

`src/persist/snapshot.rs` — placement round-trips through the snapshot, and old
snapshots without the field still restore.

`src/app/mod.rs` — `reload-config` applies the config value, but leaves a
runtime toggle alone.

### Free prefix bindings

As of `v0.7.4`/`d4e0dd3`, unused by any default — useful when adding another
action to this patch set:

- unshifted: `prefix+a`, `d`, `f`, `i`, `m`, `t`, `u`, `y`
- shifted: `prefix+shift+a`, `b`*, `c`, `e`, `f`, `i`, `m`, `o`, `q`, `s`, `u`, `v`, `y`, `z`

`*` `prefix+shift+b` is now taken by `toggle_sidebar_layout`. Check with the
`default_keymap_has_no_conflicting_bindings` test rather than by eye.

### Possible follow-ups

- Cross-space agent triage (the mobile switcher reused on desktop).
- Keyboard parity: `focus_agent = "prefix+alt+1..9"`, `next_agent`,
  `previous_agent` are layout-independent and already work, but are unbound.
- On master only, 0.7.5's `agent.view.set` defines sidebar/mobile/mouse/keybind
  ordering; the rollup and picker could honor it to inherit filtering.
- The vertical rail's `│` and the collapsed rail divider have the same
  `surface_dim` contrast problem. Left alone — pre-existing upstream behavior,
  and changing shared vertical rendering is out of scope for this patch set.

---
