// TypeScript mirror of hate-core's Event enum (crates/core/src/event.rs).
// Serde emits `{"type": "<snake_case variant>", ...fields}` — keep this file
// in sync; the Rust enum is the source of truth.

export interface TokenUsage {
  input: number;
  cached_input: number;
  output: number;
  thinking: number;
}

export interface ToolCall {
  id: string;
  name: string;
  args: Record<string, unknown>;
}

export interface PermissionRequest {
  id: string;
  call: ToolCall;
  reason: string;
  detail?: string | null;
}

export interface TurnOutcome {
  outcome: string;
  usage: TokenUsage;
  tool_calls: number;
  elapsed_ms: number;
  cost_usd: number | null;
}

// Serde's default externally-tagged enum shape: {"Provider": {…}}.
export type FailureReason =
  | { LimitExceeded: { limit: string; value: number } }
  | { Provider: { message: string } }
  | { Tool: { name: string; message: string } }
  | { PermissionDenied: { tool: string } }
  | { Internal: { message: string } };

export type SessionEvent =
  | { type: 'delta'; text: string }
  | { type: 'thinking_delta'; text: string }
  | { type: 'tool_started'; call: ToolCall }
  | { type: 'tool_output'; call_id: string; preview: string; tokens: number }
  | { type: 'tool_finished'; call_id: string; ok: boolean; full_tokens: number; sent_tokens: number }
  | { type: 'permission_requested'; request: PermissionRequest }
  | { type: 'tokens_counted'; usage: TokenUsage; turn: number; thinking_est?: number }
  | { type: 'compaction_started'; reason: string; before: number; after: number }
  | { type: 'checkpoint_created'; id: string }
  | { type: 'skill_saved'; name: string; title: string; bytes: number }
  | { type: 'turn_complete'; outcome: TurnOutcome }
  | { type: 'failed'; reason: FailureReason }
  | { type: 'error'; error: { kind?: string; message?: string } }
  | { type: 'awaiting_user'; prompt: string }
  | { type: 'done'; outcome: string };

// ── UI view models ────────────────────────────────────────────────────────

export interface TranscriptBlock {
  id: number;
  kind: 'user' | 'text' | 'thinking' | 'tool' | 'error';
  text?: string;
  streaming?: boolean;
  call?: ToolCall;
  ok?: boolean;
  fullTokens?: number;
  sentTokens?: number;
  preview?: string;
  expanded?: boolean;
  /** Usage of the provider request that produced this block (assistant
   *  text, thinking, or — for a tool-only request — its first tool card). */
  usage?: TokenUsage;
  /** `thinking` blocks: reasoning tokens for this request. */
  thinkingTokens?: number;
  /** True when `thinkingTokens` is a local count because the provider
   *  bills thinking inside output instead of reporting it. */
  thinkingEstimated?: boolean;
  /** `user` blocks: whole-turn rollup, filled when the turn completes. */
  turn?: TurnSummary;
  /** `user` blocks: this message was sent as a plan-mode turn. */
  plan?: boolean;
  /** `text` blocks: written by the app (command feedback, notices),
   *  not by the model. */
  local?: boolean;
}

export interface TurnSummary {
  usage: TokenUsage;
  toolCalls: number;
  elapsedMs: number;
  costUsd: number | null;
}

export interface PlanStep {
  description: string;
  status: 'pending' | 'active' | 'done' | 'failed';
}

export interface CheckpointEntry {
  id: string;
  label: string;
  at: string;
}

export interface ChangedFile {
  path: string;
  added: number;
  removed: number;
}

export interface FileNode {
  name: string;
  path: string;
  kind: 'dir' | 'file';
  status?: 'M' | 'A' | 'D' | '?';
  children?: FileNode[];
}

export interface SymbolHit {
  name: string;
  path: string;
  line: number;
  kind: 'def' | 'ref' | 'call';
}
