# Dependencies

Every dependency earns its place with a three-line justification, written
before it is added. Format: crate — why it exists, why nothing simpler
works, what it costs. Rows are added when the dependency lands, not before.

## Workspace (M0)

- `tokio` (rt-multi-thread, macros, fs, process, io-util, sync, time) —
  async runtime; provider streaming and `spawn_blocking` file/process work
  require it. The ecosystem standard; no lighter alternative covers the
  same surface. Cost: large dep tree, accepted.
- `serde`, `serde_json` — domain types and every wire format (provider
  APIs, Tauri IPC, config, ledger rows). Ubiquitous and effectively
  mandatory in this stack. Cost: none meaningful.
- `thiserror` — error enums for library crates per conventions; compile-time
  only, zero runtime cost. `anyhow` reserved for the app/bin layer.
- `tracing`, `tracing-subscriber` — per-turn and per-tool spans mandated by
  the architecture; structured, filterable, zero-cost when disabled.
- `uuid` — `SessionId`/`TurnId`/`CheckpointId`; v4 + serde.
- `dirs` — cross-platform config/data directories (`%APPDATA%` on Windows,
  `~/.config` on Linux, `~/Library` on macOS). One call replaces
  hand-rolled per-OS path logic.
- `toml` — `config.toml` parsing; the only non-secret config format we
  support, so one parser is enough.
- `clap` — `hate` CLI argument parsing in M1; derive API keeps it to a few
  lines.
- `reqwest` (rustls-tls, stream, json) — all provider and auth HTTP/SSE.
  rustls avoids OpenSSL build pain on Windows. Cost: TLS stack size,
  accepted.
- `futures` — stream/SSE combinators that turn provider byte streams into
  `Delta` items.
- `tokio-util` — `CancellationToken` for the 250ms cancel requirement.
- `keyring` — OS credential store (Windows Credential Manager, macOS
  Keychain, Secret Service). The brief mandates keychain storage; this is
  the standard crate.
- `webbrowser` — launch the system browser for the Nous device flow;
  tiny, cross-platform, actively maintained. Cost: one small dep,
  justified by UX. (Supersedes the originally-planned `open` crate.)
- `tiktoken-rs` — real token counts for OpenAI-compatible and local models
  (o200k_base approximation, the brief-sanctioned fallback where no
  provider counter exists). Chosen over `chars/4` estimation, which the
  spec forbids.
- `rusqlite` (bundled) — `ledger.db` storage; bundled feature removes any
  system SQLite dependency.
- `time` — ledger timestamps; smaller surface than `chrono`.
- `fastrand` — retry jitter; ~1KB, no `rand` complexity needed.
- `regex` — ANSI stripping, output shaping, and the interim `symbol`
  implementation before tree-sitter lands.
- `dunce` — `std::fs::canonicalize` returns `\\?\` verbatim paths on
  Windows; dunce simplifies them for the path jail.
- `similar` — diff generation for patch preview and edit stats (M3).
- `ignore`, `globset` — `.gitignore`-honoring file walking for `find` and
  later the index; this is what ripgrep uses under the hood.
- `grep-searcher`, `grep-regex` — ripgrep-class line search in pure Rust
  for `grep` and the interim `symbol` (M9 hardening).
- `tree-sitter` + per-language grammar crates — symbol extraction for
  `symbol`/`repo_map` (M9 only; the heaviest deps in the tree, scoped to
  one milestone).
- `rmcp` — official Rust MCP SDK; hand-rolling JSON-RPC over stdio +
  streamable HTTP with lazy lifecycle is strictly worse (M10).
- `tauri`, `tauri-build` — app shell only; every crate it wraps is
  Tauri-free by design.
- `tauri-plugin-dialog` — native folder picker for attaching a code
  workspace; hand-rolling platform dialogs is strictly worse. Used
  Rust-side only (`blocking_pick_folder`), so no JS dep is needed.
- `m5x7` + `Alagard` (fonts, not crates) — pixel faces for the landing
  wordmark, plate buttons, and pixel text. m5x7 is CC0 (Daniel Linssen);
  Alagard is free with credit (Hewett Tsoi —
  https://hewett.tsoy.net/alagard). Vendored TTFs under
  `apps/desktop/src/assets/fonts/`; attribution satisfied here.
- Landing pixel-art sprites (not crates) — vendored under
  `apps/desktop/src/assets/sprites/`:
  - `campfire.png` — Pixor campfire sheet (8 sides × 6 Burn frames,
    front row used), CC0 — https://pixor.art/library/elements/campfire/
  - `sleeping-cat.png` — user-supplied artwork (trimmed to content)
- `tauri-specta`, `specta`, `specta-typescript` — generates TypeScript
  types from the Rust command signatures, satisfying the typed-invoke-wrapper
  requirement without a hand-maintained parallel type file. Pinned to the
  `2.0.0-rc` line (`specta =2.0.0-rc.26` to match `tauri-specta`'s exact
  requirement, `specta-typescript 0.0.13` for compatibility); revisit the
  pins when specta 2.0.0 stable ships.

## Rejected alternatives

- `async-openai`, `anthropic`, `genai` SDKs — the provider layer needs
  exact control over SSE deltas, `cache_control` breakpoints, and tool-call
  accumulation; SDKs hide the wire and add weight. Hand-rolled adapters,
  one file each.
- `git2` — checkpointing shells out to the `git` CLI; `GIT_DIR` /
  `GIT_WORK_TREE` map directly onto CLI env vars, and git is already a
  de-facto requirement for the repos this agent works in.
- `schemars` — tool input schemas are explicit `serde_json::json!`
  literals; generating them adds a dep for no runtime benefit.
- `async-stream` — boxed `Stream`/`Future` return types cover the same
  need with zero extra deps.
- Any UI component library — the design system is ~25 hand-written
  components; a library fights the dense UI (spec §21).
