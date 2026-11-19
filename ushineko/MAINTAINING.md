# Maintaining this fork

## Never upstream

Nothing from this fork is offered to herdrdev/herdr: no pull requests, no
issues, no discussions. herdr's `CONTRIBUTING.md` closes unsolicited pull
requests and tells coding agents to refuse to open them. In local checkouts:

- `gh repo set-default ushineko/herdr`, so `gh pr create` targets this fork.
- The `upstream` remote has a disabled push URL.
- Always pass `--repo ushineko/herdr` and an explicit `--base` to `gh pr create`.

## Branches

| Branch | Content |
| --- | --- |
| `ushineko/features` | Integration: the herdr release tag plus every merged feature. Builds come from here. |
| `ushineko/spaces-bar` | Horizontal spaces bar ([spec 000](https://github.com/ushineko/herdr/blob/ushineko/spaces-bar/ushineko/specs/000-spaces-bar.md)). |
| `ushineko/dock` | Dock ([spec 001](https://github.com/ushineko/herdr/blob/ushineko/dock/ushineko/specs/001-dock.md)), on top of the spaces bar. |
| `ushineko/pane-stacks` | Nested tabs ([spec 002](https://github.com/ushineko/herdr/blob/ushineko/pane-stacks/ushineko/specs/002-nested-tabs.md)), on top of the dock. |

The features are stacked because they touch the same client layout code. Each
one is a pull request into the branch below it, and the bottom one targets
`ushineko/features`. Review fixes are commits on the feature's own branch.

## Moving to a new herdr release

Track release tags only, not upstream `master`, which churns too fast to carry
patches against.

1. `git fetch upstream --tags`, then create the new integration branch from
   the tag, with this fork's `ushineko/` documents and `.claude/` guard
   re-applied on top.
2. Rebase each feature branch, bottom up, onto the branch below it.
3. Run the validation below on each branch.
4. Merge into the new integration branch and tag `ushineko-v<version>.1`.

Before trusting a port plan, check which architecture the new base has. In
0.9.0, herdr moved the whole TUI into the client (`src/client/shell/`), and the
spaces bar had to be rewritten rather than rebased. A large upstream refactor
means a port, not a rebase.

## Building

```bash
ZIG=/path/to/zig-0.16.0/zig cargo build --release --locked
```

The vendored libghostty-vt pins an exact Zig version: **0.16.0** for herdr
0.9.x (0.15.2 for 0.7.x and 0.8.x). Distribution packages often ship a
different Zig, which fails in `build.rs`.

## Validation

Clear herdr's environment first. Run from inside a herdr pane, the
integration tests otherwise talk to the live server:

```bash
env -u HERDR_ENV -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
    -u HERDR_PANE_ID -u HERDR_TAB_ID -u HERDR_WORKSPACE_ID just ci
```

- `just lint` (fmt and clippy `-D warnings`) and the nextest suite must pass.
- The config reference must list every new config key
  (`scripts/test_config_reference_check.py`).
- `just ci` also runs `bun` recipes for upstream's TypeScript workers. The
  patches here do not touch those workers.
- `just windows-lint` needs `xwin` and acceptance of Microsoft's SDK license
  (`just setup-windows-cross`).
- Known: the live docs translation-parity check
  (`scripts/docs_translation_parity.py`) fails, because the English
  `configuration.mdx` has feature sections the translations lack.
- Behavioural tests are checked to fail with their behaviour removed before a
  feature is called complete.

## Compatibility rules for patches

- Endpoint protocol: add behaviour through named endpoint controls and
  capabilities, never new variants of the frozen bincode enums
  (`ClientMessage`, `ServerMessage`, `PaneSurfaceFrame`). New JSON fields
  must be optional with serde defaults; new enums need an `Unknown` fallback.
- Saved sessions: new fields must be optional, and `SNAPSHOT_VERSION` stays
  where upstream has it, so stock herdr can still load the file.
- Shared state belongs on the server and, where practical, in the JSON API,
  not only behind the private client socket (herdr's `AGENTS.md`).
