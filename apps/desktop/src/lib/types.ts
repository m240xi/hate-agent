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
  usage?: TokenUsage;
  thinkingTokens?: number;
  thinkingEstimated?: boolean;
  turn?: TurnSummary;
  plan?: boolean;
  local?: boolean;
  pins?: PinRef[];
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
  status?: 'M' | 'A' | 'D' | '?';
}

export interface PinRef {
  path: string;
  start?: number | null;
  end?: number | null;
}

export interface FileView {
  path: string;
  lines: string[];
  start: number;
  total_lines: number;
  truncated: boolean;
  binary: boolean;
  too_large: boolean;
}

export interface FileDiff {
  path: string;
  unified: string;
  added: number;
  removed: number;
  tracked: boolean;
  truncated: boolean;
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
