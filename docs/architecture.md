# hate-agent architecture

A local-first desktop coding agent. The product promise is **fewer tokens
per task** than mainstream agents on the same model — cheaper, faster, and
legible: every token is attributable to a turn, a tool, and a reason.

The differentiator is architecture, not a smarter model: a small prompt, a
lazily-loaded tool surface, aggressive output shaping, cache-stable
prefixes, and a verification loop that pushes deterministic work out of the
model and into Rust.

## Non-goals (v1)

Not a chat app. No cloud sync, accounts, or team features. No mobile or
web build. No plugin marketplace — MCP is supported but never installed by
default. No agent-authored UI.

## Layout

```
crates/
  core/            Session state machine, turn assembly, event types
  prompt/          System prompt fragments, cache-breakpoint planner
  context/         Token accounting, truncation, compaction
  tools/           Tool trait, registry, core tier implementations
  tools-deferred/  Lazily loaded tools, one module each
  providers/       Adapters + Delta normalization
  auth/            Keychain, OAuth flows, credential refresh
  index/           tree-sitter symbol map, ripgrep wrapper, ranking
  patch/           Diff generation, fuzzy match, patch application
  checkpoint/      Git shadow repo, snapshots, rewind
  policy/          Path jail, command classification, egress rules
  ledger/          Local token/cost accounting
  mcp/             Client, lazy server lifecycle
  app/             Tauri commands, events, window mgmt
apps/
  desktop/         Svelte 5 frontend
  cli/             Headless binary
bench/
  tasks/           Task suite, one dir each with spec + verify script
  runner/          Harness
```

All agent logic lives in Rust crates with no Tauri dependency. The Tauri
layer (`crates/app`) is a thin command/event adapter, under ~600 lines
total. The same core backs the headless `hate` CLI, which is what the
benchmark harness drives. If a feature only works in the GUI, it is not
done.

## Core model

`SessionHandle` accepts commands and emits a typed `Event` stream. The UI
is a pure consumer of that stream; nothing in the UI decides agent
behavior.

```rust
pub struct SessionId(Uuid);
pub struct TurnId(u32);

pub enum SessionState {
    Idle,
    Planning { plan: Plan },
    AwaitingModel,
    ExecutingTool { call: ToolCall, started: Instant },
    Verifying { attempt: u8 },
    AwaitingPermission { request: PermissionRequest },
    AwaitingUser,
    Compacted { at_turn: TurnId },
    Done { outcome: Outcome },
    Failed { reason: FailureReason },
    Cancelled,
}

pub struct ToolCall {
    pub id: String,
    pub name: ToolName,
    pub args: serde_json::Value,
}

pub enum Event {
    Delta { text: String },
    ThinkingDelta { text: String },
    ToolStarted(ToolCall),
    ToolOutput { call_id: String, preview: String, tokens: u32 },
    ToolFinished { call_id: String, ok: bool, full_tokens: u32, sent_tokens: u32 },
    PermissionRequested(PermissionRequest),
    TokensCounted(TokenUsage),
    CompactionStarted { reason: CompactionReason },
    CheckpointCreated { id: CheckpointId },
    TurnComplete(TurnOutcome),
    Error(AgentError),
}
```

Every event is `serde::Serialize`. `full_tokens` vs `sent_tokens` on tool
output is what makes the savings legible — record it or the dashboard is
fiction.

## The agent loop

States advance only on explicit transitions. A turn:

1. `TurnBuilder` assembles the request: static prefix, cached tool block,
   project instructions, compacted history, new user message.
2. Stream from the provider. Accumulate text and tool calls.
3. If no tool calls: run the verifier. Pass → `Done`. Fail → feed the diff
   of failures back and re-enter.
4. If tool calls: classify each through `policy`. Auto-approved ones run in
   parallel; gated ones pause at `AwaitingPermission`.
5. Execute, shape output, truncate, append as `tool_result`.
6. Emit usage. If `context.tokens > compact_threshold`, compact before the
   next request.
7. Loop.

Hard limits, all configurable: `max_turns_per_task = 40`,
`max_tool_calls_per_turn = 8`, `max_wall_clock = 20 min`,
`max_cost_per_task` in USD. Hitting a limit produces `Failed` with a
reason, never a silent stop.

Cancellation is `CancellationToken`-based and must interrupt an in-flight
HTTP stream within 250ms.

### Plan mode

`enter_plan` flips the loop into read-only. Tool calls are restricted to
`read`, `grep`, `find`, `symbol`; the model must emit a structured
`Plan { steps: Vec<Step> }` before `exit_plan`. The system prompt fragment
changes at one controlled cache break.

## Prompt architecture and cache discipline

The request is assembled as ordered segments, most-stable first:

```
[S0] static system prompt          ← never changes, ever
[S1] tool definitions (core tier)  ← changes only on tool_search
[S2] environment facts             ← date, OS, shell, cwd  (NOT git status)
[S3] project instructions          ← AGENTS.md etc.
[S4] conversation history
[S5] new user message
```

Hard invariants:

- S0 is a single `include_str!` constant. No interpolation.
- S2 holds only values stable for the process lifetime. Git branch and
  dirty state go in S4 as a tool result, never S2.
- Tool definitions serialize from a fixed `Vec` in fixed (alphabetical)
  order. A `tool_search` load appends to the end of the core list and
  re-marks the breakpoint — the one sanctioned prefix mutation, at most
  twice per session.
- Cache breakpoints: one at end of S2, one at end of last-but-one message.
  Anthropic gets `cache_control: ephemeral`; OpenAI-compatible relies on
  automatic prefix caching — keep the prefix byte-identical.
- `cache_hit_ratio` is logged per turn; below 0.5 outside a compaction
  turn is a regression.

System prompt budget: **≤ 1,400 tokens**, enforced by a unit test with a
real tokenizer. Content is a priority list, not a personality:

1. Do the smallest correct thing.
2. Navigate before reading; read before rewriting.
3. One tool call per decision when parallelizable, one decision per call
   otherwise.
4. Never re-read a file already in context unless it changed.
5. Verify before claiming done.
6. Match surrounding style; no drive-by reformatting.
7. Ask only when a wrong guess costs a rewrite.

## Tool surface

### Tier 1 — always loaded (8 tools + `ask_user`)

`read`, `edit`, `write`, `bash`, `grep`, `find`, `symbol`, `tool_search`.

Every tool declares `{ name, description ≤ 120 chars, input_schema,
output_budget_tokens, side_effect_class }`.

- `read`: `{path, start?, end?, mode: head|range|tail}` — line numbers
  always, head = first 120 lines + `… N more lines`, cap 400 lines / 6k
  tokens.
- `edit`: `{path, old, new}` — `old` must appear exactly once; on 0 matches
  retry whitespace-normalized and report closest region; on >1 fail with
  offsets. Success output is `patched <path>:+A -D` only.
- `grep`: `{pattern, path?, glob?, context?}` — cap 60 matches,
  `path:line: text` grouped by file.
- `bash`: `{command, cwd?, timeout_s?}` — combined output capped 80 lines /
  3k tokens, middle elided, tail preserved, exit code on its own line,
  120s default timeout.
- `symbol`: `{query, kind: def|ref|call}` — tree-sitter index; returns
  `path:line: signature`.

### Tier 2 — deferred

`glob_many`, `web_search`, `web_fetch`, `tree`, `outline`, `deps`,
`screenshot`, `tasks`, `checklist`, `spawn_agent`, `enter_plan`/`exit_plan`,
`apply_patch`, `git_diff`, `git_log`.

`tool_search` takes a query, ranks deferred tools by name+description
similarity (BM25 over ~40 docs, no embeddings), loads top 3. Loaded tools
persist for the session; cap 16 total.

## Context management

`TokenCounter` has a real implementation per provider: Anthropic
`count_tokens` (free), OpenAI `tiktoken-rs`, Gemini `countTokens`, local
`tiktoken-rs` `o200k_base` approximation. Never `chars / 4` when a real
counter exists.

Compact at 0.72 of window, hard-stop at 0.92. Compaction is triggered,
never on a timer:

1. Pin a fact block: files touched, plan, open failures, user constraints —
   carried verbatim.
2. Summarize history into `Decisions` / `State` / `Open`, hard budget 700
   tokens, on the **cheap** model.
3. Drop tool results older than the last checkpoint except pinned refs.
4. Rebuild history as `[fact block][summary]`.
5. Emit `CompactionStarted` with before/after counts.

Per-tool-result shaping runs before anything reaches the model: strip
ANSI, collapse blank runs, dedupe repeated log lines to `line (×N)`, drop
stack frames below the first project-local one.

## Repo intelligence

- `ignore`-crate walker honoring `.gitignore`; skip binaries and >1MB files.
- tree-sitter parsers: Rust, TypeScript, TSX, JavaScript, Python, Go, Java,
  C#, Ruby, PHP, C/C++. Symbols into `HashMap<SymbolName, Vec<Location>>`
  with reverse index; incremental rebuild debounced 400ms on save.
- `repo_map` (top-level dirs, entry points, per-file symbol counts) capped
  at 800 tokens, injected as a session-start tool result — never in the
  prefix.
- No embeddings in v1: BM25 over symbol names + path heuristics.

## Verification

The agent does not decide whether it succeeded.

1. Discover check commands from `AGENTS.md`, `package.json` scripts,
   `Cargo.toml`, `Makefile`, in that order.
2. First use: run each, store the baseline failure set.
3. After claimed completion: re-run, diff against baseline. Only newly
   introduced failures go back, each with its first 6 lines.
4. Zero new failures → `Done`. Otherwise loop with failures as the only
   new context.

A cheap second-opinion reviewer subagent runs before surfacing diffs over
200 lines — output is a 5-line verdict.

## Providers

`trait Provider` with `stream(req) -> Stream<Delta>`, `count_tokens`,
`list_models`, `capabilities() -> { vision, tools, cache, thinking }`.

Ship: Anthropic, OpenAI, Gemini, DeepSeek, OpenRouter, and a generic
OpenAI-compatible adapter (Ollama/LM Studio/vLLM/llama.cpp). Route by
capability, not vendor name. Unknown model → OpenAI-compatible + warning.

Retry: 3 attempts, exponential with jitter, only on 429/5xx/connect,
honor `Retry-After`. On 429 surface reset time in the UI.

Mid-conversation model switch preserves the transcript, re-tokenizes
against the new counter, and re-evaluates the compaction threshold
immediately.

## Auth

API keys in the OS keychain. `~/.config/hate-agent/config.toml` (0600)
holds only non-secret config. OAuth where vendors allow; the Nous Portal
device-code flow is the reference implementation. Every outbound payload
passes a redactor (`sk-`, `ghp_`, AWS keys, `PRIVATE KEY` blocks) with a
config allowlist for paths to skip.

## Security

- **Path jail**: canonicalize every path; reject anything escaping the
  workspace root after symlink resolution. Extra roots only via `/add-dir`.
- **Command classification**: `bash` commands parsed (not grepped) into
  `Safe` / `Write` / `Network` / `Destructive`. Destructive always prompts.
- **Egress**: default-deny network in `bash` except allowlisted package
  managers.
- **Trust prompt** on first open of any repo listing discovered MCP
  servers and hooks. Nothing executes before trust.
- **Diff preview** for every `write` and multi-hunk `edit` over 30 lines.
- Secrets never enter the transcript unless a file matches a secret-ish
  path pattern.

## Checkpoints and rewind

Shadow git repo at `<data>/checkpoints/<session>/` (`GIT_DIR` pointed
there, worktree at the real workspace). Snapshot before every
`Write`-class tool call. `rewind` restores file state, transcript state,
or both, independently. `bash` mutations are not captured — the UI says
so. Storage capped at 500MB with LRU eviction.

## MCP

JSON-RPC over stdio and streamable HTTP. Configs in
`~/.config/hate-agent/mcp.json` or `<repo>/.mcp.json`, same `mcpServers`
shape as Claude Code. MCP tools are always tier-2, namespaced
`mcp__server__tool`, loaded only via `tool_search`. Servers start lazily
on first use, idle-stop after 5 minutes.

## Subagents

`spawn_agent` runs an isolated child session: fresh prefix, no parent
history, parent supplies task + tool allowlist. Child returns
`SubagentReport { summary ≤ 400 tokens, files_touched, ok }`. Parent sees
only the report; child transcript inspectable but never enters parent
context. Depth limit 1, up to 4 parallel, each with its own token budget.
Use for broad exploration, large-file summarization, and review — not
single-file edits.

## UI

Three collapsible resizable panes: file tree + symbol search (left),
streaming transcript with one-line collapsible tool cards (center), task
inspector with plan/token meter/checkpoints/changed files (right).
Composer: Enter sends, Shift+Enter newline, Esc cancels, `/` command
palette, `@` file fuzzy-find, Shift+Tab cycles thinking level. Inline
permission bar — never `confirm()`.

Dense, quiet, keyboard-first. One accent color, 4-step grayscale, system
font + mono fallback, 6px radius, 1px borders, no shadows except composer,
no gradients. 120ms ease-out motion on enter only. Dark and light
first-class. First paint <400ms, transcript virtualized past 200 messages,
pin-to-bottom scroll.

## Token and cost ledger

Local SQLite at `<data>/ledger.db`, never synced. Per turn: input, cached
input, output, thinking, tool tokens sent vs produced, model, latency,
wall clock. Per task: rollup + USD from a bundled versioned price table
("price table stale" warning after 60 days). Live per-task bar plus a
history view with 30-day rolling median cost-per-task. The number that
matters is `tokens_per_completed_task`.

## Benchmark

`bench/` is a first-class artifact: 12 tasks across owned repos (bug fix,
refactor, feature, tests, docs, dep bump, perf, type-error cascade, 4
multi-file), each with `spec.md` + `setup.sh` + `verify.sh`. Runner does
N=5 headless runs per agent, records time/tokens/cost/pass, writes JSON.
Baselines: this agent plus Claude Code and Gemini CLI headless modes,
model pinned and reported. 3-task smoke on every PR, full suite nightly,
reports committed to `bench/results/`.

## Testing

Unit tests on pure logic (patch matching, truncation, counters, policy,
compaction golden files). Integration via `MockProvider` replaying
recorded streams — no live API in CI except one nightly job. `insta`
snapshots for every prompt segment and tool output shape; S0 changes
require `cargo insta review`. One CLI e2e per milestone on a fixture repo.
70% coverage floor on `core`, `patch`, `context`, `policy`.

## Conventions

`rustfmt` defaults, `clippy::pedantic` with documented allowlist,
`thiserror` errors, `tracing` spans per turn/tool. TypeScript `strict`,
no `any`, no default exports, `const` over `let`. `#![deny(unsafe_code)]`
everywhere except a documented allowlist. Commits `area: imperative`.
`CHANGELOG.md` generated at release.

## Performance budgets

| Metric | Budget |
|---|---|
| Cold start to usable | 900ms |
| Prefix tokens, idle session | ≤ 2,100 |
| Tool schema tokens, 0 deferred loaded | ≤ 700 |
| Frontend JS gzipped | ≤ 900KB |
| Idle RSS | ≤ 220MB |
| Cancel latency | ≤ 250ms |

## Forbidden patterns

1. Timestamps, git status, or random IDs in the cached prefix.
2. Whole-file rewrites where an edit would do.
3. Re-reading an unchanged file already in context.
4. MCP servers installed or started by default.
5. Tool schemas for deferred tools in the prefix.
6. Sending raw command output over 3k tokens.
7. A "let me explore" turn with >4 reads and no writes.
8. `chars / 4` token estimation when a real counter exists.
9. Blocking the async runtime on file or process I/O.
10. Agent logic in the Tauri layer.
11. Stubs merged as complete.
12. Emoji in the UI chrome.
13. Unbounded retry loops.
14. Silently swallowing a provider error.
