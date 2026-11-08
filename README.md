# hate-agent

A local-first desktop coding agent that does the same repository work as
mainstream agents — reading, editing, running checks, verifying — at
materially fewer tokens per task. The differentiator is architecture, not a
smarter model: a small prompt, a lazily-loaded tool surface, aggressive
output shaping, cache-stable prefixes, and a verification loop that pushes
deterministic work out of the model and into Rust.

Every token spent is attributable to a turn, a tool, and a reason — and
visible in the UI.

> Status: early development. See `docs/architecture.md` for the design and
> the milestone plan.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.
