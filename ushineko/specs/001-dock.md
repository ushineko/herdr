# 001 — herdr dock: a global pane area at the bottom

> Personal fork patch. Not part of herdr; not affiliated with the herdr project.

## Status: COMPLETE

## Context

The fork's patch set (see `ushineko/README.md`)
adds features on top of upstream releases. Today, programs that should be
visible everywhere, such as the hayami usage monitor
(`hayami-tui --sections usage --arrangement row`), are opened as a
`panel:usage` pane in every tab that uses the panel layout
(a per-tab panel script). That duplicates processes
and crowds every tab.

A **dock** is a pane area at the bottom of the screen that belongs to no
workspace. It is the same in every workspace and tab, its programs run once per
server, and inside it panes behave like panes: they split, resize and close.

Upstream has received similar requests (#917 pinned background panes, #3775 and
#529 pane-local tabs). All were closed under the bugs-only issue policy, not
rejected on merit. As with the spaces bar, this stays a local patch, never an
upstream PR.

Base: branch `ushineko/spaces-bar`
(tag `v0.9.5` plus the spaces-bar commits). The dock is a separate commit
series on top of it, so it can be dropped on its own.

Out of scope here: tabs inside the dock. That waits for the separate nested-tabs
feature.

## Requirements

Decided with the user on 2026-11-18:

1. **Position:** a strip at the bottom spanning the full terminal width, below
   the tab bar and panes. With the vertical sidebar, the sidebar ends above the
   dock.
2. **Acts like a pane area.** The dock has its own split layout. Panes in it can
   be split right or down, resized by dragging dividers or with resize mode,
   zoomed (within the dock) and closed, all with the normal pane keys while the
   keyboard is in the dock.
3. **Resizable height**, by dragging the dock's top edge or with resize mode.
   The height is **shared**: stored on the server, the same for every client.
4. **Config seeds, runtime wins.** `[[ui.dock.panes]]` builds the dock only when
   no saved dock exists. After that, the user's splits, sizes and closes are
   saved with the session and restored. A "reset dock to config" action
   rebuilds it from config. The config seed describes a single row of panes,
   left to right; other arrangements are made at runtime and saved.
5. **Focusable.** `focus_dock` moves the keyboard into the dock and back.
   Clicking a dock pane focuses it. Direction keys never move focus between the
   dock and the workspace.
6. **Per-client visibility.** `toggle_dock` hides or shows the dock on one
   client and is remembered there. Hiding does not stop the programs.
7. **No second multiplexer.** Only herdr itself.

### Config shape (proposed)

```toml
[ui.dock]
rows = 3                      # seed height in content rows; a rule row is added above

[[ui.dock.panes]]             # seed layout: one row of panes, left to right
label = "usage"
command = "hayami-tui --sections usage --arrangement row"
cwd = "~"                     # optional; default $HOME
width = 3                     # optional relative weight; default 1

[[ui.dock.panes]]
label = "net"
command = "hayami-tui --sections bandwidth --arrangement row"

[keys]
focus_dock = "prefix+u"          # proposed; checked free by the keymap test
toggle_dock = "prefix+shift+u"   # proposed; per-client show/hide
reset_dock = ""                  # unbound by default; also on the dock's right-click menu
```

With no `[[ui.dock.panes]]` and no saved dock, there is no dock: zero rows, and
no behaviour change from the spaces-bar build.

## Design

### Model: a tab that belongs to no workspace

- herdr's split layout engine (`TileLayout`, `src/layout.rs`) is
  self-contained: split, ratios, divider geometry and directional focus. The
  dock owns one, plus its own pane records:
  `app.state.dock: Option<DockState { layout: TileLayout, panes: Map<PaneId, DockPane>, rows: u16, rows_source, zoomed }>`.
- Each `DockPane` has a `terminal_id`, and an origin:
  - **Seeded:** from config, with `label`, `command`, `cwd`. Respawned with
    backoff if its program exits, as long as it has not been closed.
  - **User:** created by a split. It runs the default shell, in the cwd of the
    pane it was split from, and closes when its program exits, like any pane.
- This mirrors the popup (`app.state.popup_pane`, `src/app/popup.rs`), the one
  terminal upstream already keeps outside workspaces. It is a sibling store,
  not a hidden workspace. A hidden workspace would reuse more code, but 183
  non-test sites enumerate `app.state.workspaces`: the sidebar, numbering, the
  API, persistence, the navigator, agent lists and close-last-workspace logic.
  Each site missed would leak the dock. A separate store fails safe: a site
  that isn't updated simply doesn't know about the dock.

### Geometry and sizing

- The server owns the dock height (`rows`), seeded from `ui.dock.rows`. It
  publishes it in the client snapshot.
- The client carves `rows + 1` lines at the bottom, full width
  (`ClientShellLayout.dock`), next to `spaces_bar`, `tab_bar` and
  `pane_surface`, and reports the dock width to the server. The carve is empty
  when there is no dock, the dock is hidden on this client, the terminal is at
  or below `ui.mobile_width_threshold`, or the active endpoint lacks the dock
  capability.
- The height is capped so the workspace keeps at least 3 rows. The cap is
  applied per client at draw time; the stored height does not change.
- With several clients, the dock width follows the most recent client to
  interact, the policy shared tabs already use (0.9.0, #3526). The height is
  identical everywhere.

### Rendering

- The server renders the dock layout the way it renders a tab
  (`render_tab_surface_virtual`, `render_terminal_virtual`) into one dock
  frame. It sends it as a `DockSurface { frame, panes, splits, rows }` in a
  named control (`endpoint.dock.surface.v1`), reusing `PaneSurfacePane` and
  `PaneSurfaceSplit`. It is **not** added to `PaneSurfaceFrame`, which is
  bincode and frozen (see the Phase 0 findings).
- The client blits it into `layout.dock` and draws the rule above it: in
  `overlay0`, or in `accent` while the dock holds the keyboard, as for the
  spaces-bar rule.
- Pane borders, the focused-pane highlight and split dividers inside the dock
  follow the user's `ui.pane_borders` setting, as in a tab.

### Input and focus

- Keyboard focus is client-side. A new `ClientInputTarget::Dock(pane)` routes
  typed keys to the focused dock pane. On the wire, dock input, the dock width
  and dock actions are named controls (`endpoint.dock.*.v1`), not new
  `ClientMessage` variants. The server's focused workspace pane,
  and so agent "seen" state, does not change.
- The server accepts dock input from any active shell client that shows the
  dock. This is the dock's counterpart to the `shell_client_views_pane` gate
  on pane input.
- `focus_dock` toggles between the workspace pane and the last focused dock
  pane. Clicking a dock pane focuses it, and clicking a workspace pane returns
  focus.
- While the keyboard is in the dock, these act on the dock's layout instead of
  the workspace's:
  - split right and split down
  - close pane
  - zoom (fills the dock area only)
  - focus left, right, up and down (they stop at the dock's edges)
  - resize mode
- In resize mode inside the dock, up/down move the dock's top edge (its height)
  when the focused pane touches that edge, and the nearest divider otherwise.
  Left/right move the nearest divider.
- Workspace-level keys still act on the workspace: new tab, workspace
  switching, navigator and the like.
- Mouse:
  - dragging the rule above the dock sets the height; double-clicking it resets
    the height to `ui.dock.rows`
  - dragging a divider inside the dock sets that split ratio
  - clicks and the wheel inside a dock pane follow the pane rules (forwarded
    when the program requests mouse reporting, otherwise scrollback)
  - right-clicking a dock pane opens a menu with split right, split down,
    close and "Reset dock to config"

### Persistence and lifecycle

- **Server start:** a saved dock in the session snapshot is restored, with its
  layout, ratios, height and pane origins. Seeded panes are respawned from
  their saved command; user panes start the default shell in their saved cwd.
  If there is no saved dock, the dock is built from config.
- **Save:** the dock is written to the session snapshot as an optional field.
  Stock herdr ignores it. This was verified in Phase 0: stock v0.9.5 restored
  a spike session and dropped the field on its next save. Rolling back loses
  only the dock layout, and on the next patched start the config re-seeds it.
- **`reload-config`** does not touch a running dock (runtime wins). New seed
  config applies through "reset dock to config", or when no dock exists.
- **Reset dock to config:** kills every dock program and rebuilds layout and
  height from config.
- **Closing panes:** closing the last dock pane leaves an empty dock with zero
  rows. That state is saved. Reset brings the dock back.
- **Respawn backoff** for seeded panes: 1 s, doubling to 30 s, and resetting
  after 60 s of uptime. Until the program returns, its pane shows
  `[label exited, restarting in Ns]`.
- **Excluded in v1:** dock terminals are left out of agent detection,
  notifications, `herdr pane list` and workspace lists.
- **SSH machines:** each server has its own dock. The client shows the dock of
  the active endpoint.
- **Live handoff** does not carry dock processes over: they belong to no
  workspace, so they are not in the handoff runtime set. The replacement
  server restarts them from the snapshot, which loses the state of shells in
  user panes. Accepted for v1, since handoff is only used for upgrades.

### Compatibility gate: Phase 0 findings (2026-11-18)

How 0.9.5 negotiates optional features:

- **Two protocols.** The private binary protocol (`PROTOCOL_VERSION = 22`,
  `src/protocol/wire.rs`) guards same-install direct-terminal and internal
  paths. Client-owned shells (the local TUI, and SSH and Cloud endpoints) use
  the **stable endpoint contract** in `src/protocol/endpoint.rs`, at
  generation 1.
- **The contract's rules,** from the module doc of `src/protocol/endpoint.rs`:
  generation 1 stays available indefinitely; new JSON fields must be optional
  or have serde defaults; unknown named controls are ignored unless
  negotiated.
- **bincode messages are frozen.** `ClientMessage`, `ServerMessage` and
  `PaneSurfaceFrame` are bincode-encoded (`write_message` / `read_message` in
  `src/protocol/wire.rs`). Bincode is positional, and their variant order is
  frozen for generation 1. New behaviour must go "through `EndpointControl`
  or advertised API methods, not new enum variants".
- **The handshake is JSON** inside `EndpointControl`. The client sends
  `endpoint.hello.v1` (`EndpointClientHello`, `src/client/handshake.rs`). The
  server answers `endpoint.welcome.v1` (`EndpointServerWelcome::compatible`,
  `src/server/client_transport.rs`), listing `methods` and `capabilities`
  strings. The client stores them in `EndpointNegotiation`
  (`src/client/endpoint/registry.rs`, `supports_capability`). Existing
  capabilities: `surface_interest`, `health_check`, `agent_view_projection`,
  `agent_completions`, and the surface encodings.
- **The snapshot is JSON** (`snapshot_message`, kind `shell.snapshot.v1`).
  None of the shell protocol types use `deny_unknown_fields`.
- **Unknown named controls are ignored on both sides.** The server logs
  "ignoring unknown endpoint control message" in
  `src/server/client_transport.rs`. The client returns `Ignored` in
  `src/client/endpoint/control.rs`, covered by upstream's
  `unknown_optional_controls_are_ignored` test. Kinds starting
  `shell.snapshot.` are mandatory, so dock kinds use `endpoint.dock.`.
- **The saved session** loads through `RawSessionSnapshot`
  (`src/persist/snapshot.rs`), which ignores unknown fields. A file whose
  `version` is above `SNAPSHOT_VERSION` (3) is refused, so the dock must not
  bump the version.

The resulting gate:

- **Opt-in and advertisement:** the server advertises a `"dock"` capability.
  A client that sees it opts in by sending its first dock control, its width.
  As built, this replaced the planned `dock: true` hello field, which would
  have meant threading a new field through 23 construction sites of
  `ClientShellConnected` for no gain.
- **Server side:** it sends dock controls only to clients that opted in.
- **Client side:** it sends dock controls only when the welcome lists `dock`.
- **Snapshot:** `ClientShellSnapshot` is not changed. The dock height travels
  in the dock control.
- **Saved session:** gains an optional `dock` field, and the version stays 3.

The spike (a throwaway branch, commit `5b2e004f`, with a cross-version
harness; deleted on 2026-11-19 at the user's request, after the
real implementation passed the same cross-version checks) checked this with real binaries, a stock v0.9.5 build
against the spike build, on throwaway sessions:

| Client → server | Renders | Errors/warnings | Dock traffic |
| --- | --- | --- | --- |
| spike → spike | yes | none | capability `dock=true`; probe answered; unsolicited push received |
| stock → spike | yes | none | the server pushed an unsolicited dock control; the stock client ignored it |
| spike → stock | yes | none | the client saw `dock=false` and sent nothing |

In the saved-session check, the spike server saved a session with
`"dock": {...}`, and a stock server restored it (`session restore evaluated
outcome=ok`, workspace `marker-ws` present). The stock server's next save
dropped the field. Rollback therefore loses only the dock layout, as the
design assumed.

## Acceptance Criteria

### Phase 0 — compatibility spike
- [x] How a 0.9.5 server and client negotiate optional features (endpoint
      generation, capabilities) is documented in this spec, with file
      references.
- [x] A test shows that a stock-v0.9.5 client and a patched server still
      connect and render, with a dock present on the server.
- [x] A test shows that a patched client and a stock-v0.9.5 server still
      connect and render, and the patched client shows no dock.
- [x] A test shows that stock v0.9.5 loads a session snapshot containing the
      dock field, with no error and with the workspaces intact.

### Config
- [x] `[ui.dock]` (`rows`, default 3) and `[[ui.dock.panes]]` (`label`,
      `command`, optional `cwd` and `width`) parse. Duplicate labels, empty
      commands, `rows = 0` and `width = 0` are rejected with a diagnostic.
- [x] `keys.focus_dock`, `keys.toggle_dock` and `keys.reset_dock` exist. The
      shipped defaults produce no keymap diagnostics.
- [x] The config reference (`docs/next/website/src/data/config-reference.json`)
      lists the new keys, and the docs contract checks pass.

### Server — model and lifecycle
- [x] With no saved dock, the server builds the dock from config: one pane
      per entry, left to right, ratios matching the width weights, height
      `rows`, each program started in its cwd.
- [x] A saved dock is restored with the same layout, ratios, height and pane
      origins. The config seed is not applied.
- [x] A seeded pane whose program exits is respawned with the backoff above,
      and shows the restart notice in the meantime. A user pane whose program
      exits is closed.
- [x] `reload-config` leaves a running dock untouched (same PIDs, same
      layout).
- [x] Reset dock to config kills all dock programs and rebuilds from config.
- [x] Closing the last dock pane leaves an empty, zero-row dock, which
      survives a restart.
- [x] Dock terminals do not appear in `herdr pane list`, workspace lists,
      agent lists or notifications.
- [x] Dock ptys are resized to their layout rectangles within the reported
      dock width and the shared height.

### Client — layout and drawing
- [x] The dock takes `rows + 1` lines at the bottom, full width. Panes, the
      tab bar (top or bottom), and the spaces bar or vertical sidebar all sit
      above it.
- [x] Without a dock, every layout rectangle is unchanged.
- [x] Below the mobile threshold, no dock is drawn and the panes get the rows.
- [x] The workspace keeps at least 3 rows when the dock is tall.
- [x] `toggle_dock` hides and shows the dock on this client only. The choice is
      saved in client chrome preferences and survives restart.

### Client — behaving like a pane area
- [x] `focus_dock` moves keyboard input into the dock and back. Typed keys
      reach the focused dock program and no workspace pane.
- [x] Clicking a dock pane focuses it, and clicking a workspace pane returns
      focus. The rule is accented while the dock has focus.
- [x] With the keyboard in the dock, split right and split down create a user
      pane in the dock (default shell, cwd of the source pane). Close, zoom
      (dock area only) and focus left/right/up/down act on the dock and stop
      at its edges.
- [x] Dragging a divider inside the dock changes that split ratio for every
      client. Resize mode left/right moves the nearest divider.
- [x] Dragging the rule changes the dock height for every client.
      Double-clicking the rule resets it to `ui.dock.rows`. Resize mode up/down
      moves the top edge when the focused pane touches it.
- [x] Mouse clicks and wheel over a dock pane follow the pane rules
      (forwarded when the program requests mouse reporting, otherwise
      scrollback).
- [x] Right-clicking a dock pane offers split right, split down, close, and
      Reset dock to config.
- [x] With an SSH machine active, the client shows that machine's dock, or no
      dock if the endpoint lacks the capability.

### Integration (real downstream systems)
- [x] An integration test starts a real server and client with a dock entry
      running a fixed-output command. It asserts that the output appears in
      the client's frame inside the dock rectangle, and that a keystroke sent
      with the dock focused reaches that terminal and no workspace pane.
- [x] An integration test splits a dock pane, restarts the server, and
      asserts that the restored dock has two panes with the saved ratio and
      height.

## Test strategy (for review before tests are written)

- **Unit (config):** parsing, validation diagnostics, a default keymap free of
  conflicts, seed-to-layout conversion (weights to ratios).
- **Unit (layout):** dock carve-out for each tab-bar position and sidebar mode,
  mobile, a hidden dock, the minimum workspace rows cap. Resize-mode
  edge-versus-divider selection.
- **Server tests** with real terminal runtimes, as `src/app/popup.rs` tests do:
  - seed, restore and reset
  - respawn with backoff for seeded panes (time injected, not slept), and close
    on exit for user panes
  - split and close inside the dock
  - reload leaving the dock alone
  - exclusion from lists
  - snapshot round trip
- **Client-shell behaviour tests** (`src/client/shell/tests/`, same harness as
  `spaces_bar.rs`), composed with fake `DockSurface`s:
  - focus routing, and the keys acting on the dock
  - drag hits for the rule and the dividers
  - the right-click menu
  - the toggle persisted in preferences
  - the accent rule
- **Compatibility tests:** encode with the new types and decode with the
  v0.9.5 types, in both directions and for the snapshot. This is the Phase 0
  deliverable.
- **Integration:** the two real server-plus-client tests above.
- **Mock boundaries:** no mocks for terminals, the wire protocol or snapshot
  files. Time is injected for the backoff only.
- **Regression guard:** each behavioural test is checked to fail with its
  behaviour removed, as the spaces-bar tests were.

## Risks & Assumptions

- **Wire and snapshot compatibility:** resolved by Phase 0 (see findings).
  Remaining risk: an upstream change to the endpoint contract, which a
  rebase would show.
- **Dock frames as JSON:** named controls carry JSON, not the bincode surface
  codec, so dock frames are larger on the wire than pane surfaces. Dock
  programs such as hayami redraw about once a second over a few rows, so this
  is expected to be negligible locally. Measure it over SSH before relying on
  it there.
- **Rollback:** `ln -sfn` to the previous binary plus a `live-handoff`. This
  loses only the dock layout; Phase 0 confirmed that stock herdr ignores the
  dock snapshot field. Recorded on 2026-11-18: with `[ui.dock]` in the config,
  stock v0.9.5 shows its non-fatal "config has unknown keys; herdr config
  check" banner, client- and server-side, and otherwise works.
- **Patch-set maintenance:** this touches the server, the protocol, the
  renderer, input routing and persistence. Each upstream release will cost
  noticeably more to rebase than the spaces bar does. It is kept as a separate
  commit series so it can be dropped alone.
- **Key-routing complexity:** the same keys mean "workspace" or "dock"
  depending on client-side focus. Every rerouted action needs a test both
  ways.
- **Multi-client width:** two clients with different widths resize the dock
  ptys back and forth, as with shared tabs. Accepted.
- **Respawn loops:** a broken seeded command restarts at most every 30 s; its
  errors go to the server log.
- **Assumption:** hayami renders correctly at whatever width the dock gives it.
  Verify at the narrow end.

## Deliberately not supported (v1)

- Tabs inside the dock. That waits for the nested-tabs feature.
- Text selection and copy mode inside dock panes.
- Agent detection and notifications for dock programs.
- Moving panes between the dock and a workspace.
- Opening dock panes from the CLI or the API.
- Dock panes in the JSON API. `pane list`, `pane read`, `pane process-info`
  and `pane run` do not see dock panes. Seeded programs need no outside help,
  since herdr restarts them, but a program started by hand in a dock split is
  not restored by herdr-resurrect. Deferred (2026-11-19) to the
  herdr-resurrect port, where the need is concrete.
- Dock positions other than bottom.
- Locking the dock height. Height stays adjustable by drag and resize mode. A
  config-time lock would make the dock harder to expand for other uses, and a
  runtime lock toggle is the same thing as pane pinning (next item).
- Pinning a pane to a fixed number of rows or columns. Splits are ratios
  everywhere (`src/layout.rs`), which is why the current per-tab panels drift
  on restore and resize. A fixed-size split is a layout-engine feature that
  would serve workspace panels and dock splits alike, so it gets its own spec
  (planned `003`; nested tabs took `002`). The dock's own height does not depend on it: that height is
  an absolute row count outside the split tree.

## Executive Summary

The dock is a pane area along the bottom of the screen that belongs to no
workspace. It is seeded from `[ui.dock]`, saved with the session, and shared
by every client. Inside it the normal pane keys split, close, zoom, focus and
resize. Reviewers should start with the compatibility gate (a `dock`
capability and named `endpoint.dock.*` controls; no frozen bincode message is
touched), then `src/app/dock.rs` (model and lifecycle) and
`src/client/shell/dock.rs` (layout, focus, keys, mouse). Branch
`ushineko/dock`: four feature commits and two test commits on top of
`ushineko/spaces-bar`.

## E2E Test Plan

Environment: the development machine (Linux x86_64).
- Steps 1–7 run on a throwaway `--session` driven through a private
  `tmux -L hztest` server. tmux here is only the test harness, the same method
  used to smoke-test the spaces bar; it is not part of the user's setup.
- Step 8 runs live, after the user approves the install.

1. Seed two dock panes (hayami usage with `width = 3`, and `htop`) in a
   throwaway config, then launch → both show side by side at the bottom, and
   the panes above shrink by 4 rows. *(Seed; layout.)*
2. Switch workspaces and tabs → the dock is unchanged and live, with one hayami
   process (`pgrep -c hayami-tui` = 1). *(Global.)*
3. `prefix+u`, then split down → a shell opens under htop, inside the dock.
   Typing reaches that shell only. *(Focus; split in the dock.)*
4. Drag the rule up by 3 rows, and drag the divider between hayami and htop →
   the sizes change. Restart the server → the layout, ratio and height are
   restored. *(Resize; persistence.)*
5. In htop, press `q` → it respawns after about 1 s. In the split shell, run
   `exit` → that pane closes. *(Seeded respawn versus user close.)*
6. `prefix+shift+u` → the dock hides on this client; restart the client → it is
   still hidden. Right-click a dock pane, then Reset dock to config → the
   original two panes and height are back. *(Toggle; reset.)*
7. Attach from hosts running 0.8.2 or 0.9.1 where possible → the
   connection works and shows no dock. *(Compatibility.)*
8. Live: move hayami from the per-tab `panel:usage` pane in
   the panel script into the dock, and use it for a day. *(Real use.)*

### Results (2026-11-19, herdr `af24f9de`, release build, throwaway `--session` under `tmux -L`)

| Step | Result |
| --- | --- |
| 1 | **Pass.** hayami (width 3) and `top` side by side under the rule; panes above shrank by 4 rows. |
| 2 | **Pass.** With 4 workspaces, the server started exactly one hayami process. |
| 3 | **Pass.** After growing the dock, `prefix+u` then `prefix+minus` started a user shell (`dock: started pane process … <shell>`). Typed text appeared in the dock only. On a 3-row dock the split shell has one row, so its output scrolls away immediately. That is expected, not a defect. |
| 4 | **Pass.** Dragging the rule up 3 rows moved it from line 27 to line 24. A server restart kept it there. The divider ratio surviving a restart is covered by the integration test. |
| 5 | **Pass.** `q` in `top` logged `pane exited, respawning … in_ms=999`, then restarted it. `exit` in the user shell closed its pane with no respawn. |
| 6 | **Pass.** A hidden dock stayed hidden across a client restart. Reset from the right-click menu started 2 processes and restored the seed height. |
| 7 | **Pass (stock v0.9.5).** A stock client attached to the dock server and rendered with no dock. 0.8.2 and 0.9.1 hosts were not attached; both speak the same endpoint generation 1 contract. |
| 8 | **Pending.** Needs the install on the daily-driver machine. |

Also observed: at the time of the run, the author's live sessions had
**18** `hayami-tui --sections usage` processes, one per tab with the panel
layout. Step 8 replaces them with one dock process.

Verification at `af24f9de`: `just lint` (fmt and clippy `-D warnings`) clean;
nextest **4116 passed**, 0 failed, including 57 dock tests and the 2 real
server-and-client integration tests in `tests/dock.rs`; the config-reference,
changelog and UI-hot-path checks OK. Each behavioural
test was checked to fail with its behaviour removed: 12 mutations, all caught.
Not run: the `bun` recipes and `just windows-lint` (see `ushineko/MAINTAINING.md`).
*Correction:* the docs-translation-parity claim above covered only the
checker's unit tests. The live check (`scripts/docs_translation_parity.py`)
failed at `af24f9de`, because the translations lacked the local sections; it
passes since the review fixes below.

### Review fixes (2026-11-19)

An independent review against herdr's `AGENTS.md` raised the following;
fixed on this branch unless stated:

- The full render redrew the dock on every pass. It now redraws only when
  `Dock::needs_redraw` is set (layout changes, spawns, exits, a restart
  countdown, palette changes); pty output keeps its own path. Test:
  `full_renders_redraw_the_dock_only_after_it_changed`.
- `DockDirection` has an `Unknown` serde fallback, ignored by the server.
- Client dock focus is released when the dock goes away, instead of in
  composition. Test: `a_dock_that_closes_gives_the_keyboard_back_without_a_redraw`.
- No `expect()` in seed building or dock key handling; the home directory
  comes from `worktree::home_dir()`.
- Japanese and Simplified Chinese configuration pages gained the dock and
  spaces-bar sections.
- Dock panes in the JSON API: deferred (see "Deliberately not supported").

Narrowed validation, and why it is safe: the `bun` recipes test upstream's
TypeScript workers and website tooling, which no dock commit touches
(`git diff --stat v0.9.5 ushineko/dock` shows Rust, the config reference and
the configuration docs only). `just windows-lint` cross-compiles clippy for
Windows and needs `xwin`; the fork's GitHub CI runs the native Windows check
and ConPTY packaging on every pull request instead.

Re-verified after the fixes: `just lint` clean; nextest **4118 passed**;
three new mutations (no redraw flag on change, no focus release, no serde
fallback), each caught. `just bench-render-scale` against `ushineko/spaces-bar`
(release builds, same machine): server render rows within run-to-run noise
(the dock branch measured 3–9% faster on dense scenes). The client navigator
profile is consistently slower: about 430 µs against 405 µs per frame, mean
of 14 cases over three interleaved runs. That profile has no dock, and every
dock path in client composition returns early without one, so the cause is
not dock work per frame. **Open:** profile it before calling the dock
cost-free on the client.

### Prod visibility

Not applicable. This is a personal workstation tool with no production
deployment. Observability is herdr's server log
(`herdr-server.log` in herdr's config directory), where dock seed, restore, respawn and
reset events are logged with pane labels.

## Alternatives Considered

- A hidden workspace for the dock: rejected because of the 183 enumeration
  sites that would each need to hide it (see Design).
- Config as the only source of truth, with runtime changes lost on restart:
  rejected by the user in favour of "config seeds, runtime wins".
- A dock height chosen per client: rejected by the user, because one pty can
  have only one size and clients would fight over it.
- Dock-specific split and close keys: rejected by the user in favour of the
  normal pane keys acting in the dock.
- A one-line `ui.tab_bar_right` `command` entry for hayami: rejected because it
  covers only a single-line readout.
- tmux or another multiplexer inside a pane: rejected by the user, because
  herdr replaced tmux and must stay the only session layer.
