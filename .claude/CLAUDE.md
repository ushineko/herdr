# ushineko/herdr fork: agent rules

This checkout is a personal fork of herdr. Upstream's `AGENTS.md` (also linked
as `CLAUDE.md`) applies in full, with one fork rule on top:

- **Never open a pull request, issue or discussion against herdrdev/herdr**, and
  never push to the `upstream` remote. Pull requests are opened only inside this
  fork: `gh pr create --repo ushineko/herdr --base <ushineko/... branch>`.
- Feature specs, the branch model and the release-rebase procedure are in
  `ushineko/` (start with `ushineko/MAINTAINING.md`).
