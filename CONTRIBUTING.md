# Contributing

Thanks for your interest. A few ground rules keep this codebase forkable:

- **Read `docs/architecture.md` first.** The token-economy invariants there
  are the product; a change that spends more tokens per turn needs a strong
  reason.
- **Work happens in milestone-sized chunks.** Each milestone ends green
  (fmt, clippy `-D warnings`, tests) with a demo. Don't merge a stub and
  call it done.
- **Dependencies are a cost.** Before adding one, write the three-line
  justification in `docs/deps.md`. If you can't, don't add it.
- **Commits**: `area: imperative summary` (e.g. `tools: cap grep output at
  60 matches`). One logical change per commit, no "wip" merges to main.
- **Tests**: unit tests on pure logic, integration tests through
  `MockProvider` — no live API calls in CI. `insta` snapshots guard prompt
  segments and tool output shapes.
- **Style**: `rustfmt` defaults, `clippy::pedantic`, `thiserror` errors,
  `tracing` spans. TypeScript is `strict` with no `any` and no default
  exports. Every public item gets a doc comment saying *why* it exists.
- **UI**: dense and quiet. No emoji in chrome, no component library, ~25
  component ceiling before review.

Questions and design discussion are welcome in issues before large changes.
