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
  diff_preview?: string | null;
}

export interface TurnOutcome {
  outcome: string;
  usage: TokenUsage;
  tool_calls: number;
  elapsed_ms: number;
  cost_usd: number | null;
  tool_full_tokens?: number;
  tool_sent_tokens?: number;
  cache_hit_ratio?: number;
  cache_break_reason?: string | null;
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
  | { type: 'image'; source: string; call_id: string | null }
  | { type: 'tool_started'; call: ToolCall }
  | { type: 'tool_output'; call_id: string; preview: string; tokens: number }
  | {
      type: 'tool_finished';
      call_id: string;
      ok: boolean;
      full_tokens: number;
      sent_tokens: number;
      full_preview?: string | null;
      sent_preview?: string | null;
    }
  | { type: 'permission_requested'; request: PermissionRequest }
  | {
      type: 'tokens_counted';
      usage: TokenUsage;
      turn: number;
      thinking_est?: number;
      cache_hit_ratio?: number;
      cache_break_reason?: string | null;
    }
  | { type: 'compaction_started'; reason: string; before: number; after: number }
  | { type: 'checkpoint_created'; id: string }
  | { type: 'skill_saved'; name: string; title: string; bytes: number }
  | { type: 'turn_complete'; outcome: TurnOutcome }
  | { type: 'failed'; reason: FailureReason }
  | { type: 'error'; error: { kind?: string; message?: string } }
  | { type: 'awaiting_user'; prompt: string }
  | { type: 'context_usage'; used: number; size: number }
  | { type: 'done'; outcome: string };

export type ContextRef =
  | { kind: 'git_diff' }
  | { kind: 'git_staged' }
  | { kind: 'terminal'; id?: string | null; lines?: number | null }
  | { kind: 'url'; url: string }
  | { kind: 'folder'; path: string };

export interface TranscriptBlock {
  id: number;
  kind: 'user' | 'text' | 'thinking' | 'tool' | 'image' | 'error';
  images?: string[];
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
  contexts?: ContextRef[];
  bookmarked?: boolean;
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
  userIndex?: number;
}

export interface ToolAuditEntry {
  callId: string;
  name: string;
  ok: boolean;
  fullTokens: number;
  sentTokens: number;
  fullPreview: string | null;
  sentPreview: string | null;
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

export interface ShellInfo {
  id: string;
  label: string;
  program: string;
  args: string[];
}

export interface TerminalTabInfo {
  id: string;
  title: string;
  shell: string;
  cwd: string;
  alive: boolean;
  exit_code: number | null;
}

export interface WorkspaceRuleInfo {
  file: string;
  active: boolean;
  bytes: number;
  preview: string;
}

export interface CustomCommandInfo {
  name: string;
  description: string;
  body: string;
  scope: string;
  path: string;
}

export interface WorktreeStatus {
  is_git: boolean;
  is_worktree: boolean;
  branch: string;
  main_root: string | null;
  worktree_path: string | null;
  dirty: boolean;
  ahead: number;
  behind: number;
}

export interface MergeBackResult {
  merged_branch: string;
  target_branch: string;
  main_root: string;
  summary: string;
}

export interface BranchSibling {
  id: string;
  title: string;
  updated_at: number;
}

export interface BranchGroup {
  fork_turn: number;
  branches: BranchSibling[];
}
