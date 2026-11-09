import type { CheckpointEntry, ChangedFile, FileNode, PlanStep, SymbolHit, TranscriptBlock } from './types';

export const fixtureTree: FileNode[] = [
  {
    name: 'src',
    path: 'src',
    kind: 'dir',
    children: [
      {
        name: 'core',
        path: 'src/core',
        kind: 'dir',
        children: [
          { name: 'session.rs', path: 'src/core/session.rs', kind: 'file', status: 'M' },
          { name: 'types.rs', path: 'src/core/types.rs', kind: 'file' },
        ],
      },
      {
        name: 'tools',
        path: 'src/tools',
        kind: 'dir',
        children: [
          { name: 'read.rs', path: 'src/tools/read.rs', kind: 'file', status: 'A' },
          { name: 'edit.rs', path: 'src/tools/edit.rs', kind: 'file' },
          { name: 'mod.rs', path: 'src/tools/mod.rs', kind: 'file', status: 'D' },
        ],
      },
      { name: 'main.rs', path: 'src/main.rs', kind: 'file' },
      { name: 'lib.rs', path: 'src/lib.rs', kind: 'file', status: '?' },
    ],
  },
  {
    name: 'crates',
    path: 'crates',
    kind: 'dir',
    children: [
      { name: 'core', path: 'crates/core', kind: 'dir' },
      { name: 'providers', path: 'crates/providers', kind: 'dir' },
    ],
  },
  { name: 'Cargo.toml', path: 'Cargo.toml', kind: 'file' },
  { name: 'README.md', path: 'README.md', kind: 'file' },
];

export const fixtureSymbols: SymbolHit[] = [
  { name: 'SessionHandle', path: 'src/core/session.rs', line: 42, kind: 'def' },
  { name: 'run_turn', path: 'src/core/loop.rs', line: 118, kind: 'def' },
  { name: 'shape_output', path: 'src/tools/output.rs', line: 77, kind: 'def' },
  { name: 'SessionHandle::send', path: 'src/core/session.rs', line: 210, kind: 'call' },
  { name: 'TokenCounter', path: 'src/context/tokens.rs', line: 15, kind: 'def' },
];

export const fixturePlan: PlanStep[] = [
  { description: 'Locate the retry logic in providers', status: 'done' },
  { description: 'Read session.rs turn assembly', status: 'done' },
  { description: 'Patch exponential backoff jitter', status: 'active' },
  { description: 'Run cargo test -p hate-providers', status: 'pending' },
];

export const fixtureCheckpoints: CheckpointEntry[] = [
  { id: 'cp-3f9a', label: 'before edit src/tools/edit.rs', at: '14:02:11' },
  { id: 'cp-81bc', label: 'before write Cargo.toml', at: '14:01:47' },
  { id: 'cp-02dd', label: 'session start', at: '14:00:58' },
];

export const fixtureChangedFiles: ChangedFile[] = [
  { path: 'src/core/session.rs', added: 34, removed: 12 },
  { path: 'src/tools/edit.rs', added: 8, removed: 3 },
  { path: 'src/tools/output.rs', added: 56, removed: 0 },
];

export const fixtureScript: Array<{ delay: number; event: unknown }> = [
  {
    delay: 300,
    event: {
      type: 'delta',
      text: "I'll trace the retry path first — locating where backoff is computed before touching anything.\n\n",
    },
  },
  {
    delay: 200,
    event: {
      type: 'tokens_counted',
      usage: { input: 2140, cached_input: 0, output: 58, thinking: 0 },
      turn: 3,
    },
  },
  {
    delay: 500,
    event: {
      type: 'tool_started',
      call: {
        id: 'call_1',
        name: 'symbol',
        args: { query: 'retry', kind: 'def' },
      },
    },
  },
  {
    delay: 700,
    event: {
      type: 'tool_output',
      call_id: 'call_1',
      preview: 'src/providers/retry.rs:14: pub fn backoff(attempt: u32) -> Duration',
      tokens: 42,
    },
  },
  {
    delay: 900,
    event: {
      type: 'tool_finished',
      call_id: 'call_1',
      ok: true,
      full_tokens: 118,
      sent_tokens: 42,
    },
  },
  {
    delay: 400,
    event: {
      type: 'thinking_delta',
      text: 'The jitter is missing — attempt * base would stampede on recovery. Adding fastrand inside backoff keeps it deterministic-testable via seed injection later.',
    },
  },
  {
    delay: 200,
    event: {
      type: 'tokens_counted',
      usage: { input: 2380, cached_input: 2140, output: 121, thinking: 0 },
      turn: 3,
      thinking_est: 34,
    },
  },
  {
    delay: 600,
    event: {
      type: 'tool_started',
      call: {
        id: 'call_2',
        name: 'edit',
        args: { path: 'src/providers/retry.rs', old: 'attempt * 250', new: 'attempt * 250 + jitter' },
      },
    },
  },
  {
    delay: 800,
    event: {
      type: 'tool_finished',
      call_id: 'call_2',
      ok: true,
      full_tokens: 220,
      sent_tokens: 12,
    },
  },
  {
    delay: 500,
    event: {
      type: 'checkpoint_created',
      id: 'cp-fixture-1',
    },
  },
  {
    delay: 500,
    event: {
      type: 'delta',
      text: 'Done. `backoff` now adds jittered noise inside the existing cap, and `cargo test -p hate-providers` passes (14 tests). The full output stayed home — only the diff summary reached the transcript.\n\nTwo things worth knowing: the change is inside the retry wrapper so callers are untouched, and the seed injection point is marked for deterministic tests.\n',
    },
  },
  {
    delay: 200,
    event: {
      type: 'tokens_counted',
      usage: { input: 2710, cached_input: 2380, output: 96, thinking: 0 },
      turn: 3,
    },
  },
  {
    delay: 300,
    event: {
      type: 'turn_complete',
      outcome: {
        outcome: 'Completed',
        usage: { input: 7230, cached_input: 4520, output: 275, thinking: 0 },
        tool_calls: 2,
        elapsed_ms: 41200,
        cost_usd: 0.0031,
      },
    },
  },
];

export const fixtureUsage = {
  input: 18940,
  cached_input: 15200,
  output: 388,
  thinking: 96,
};

export const fixtureInitialBlocks: TranscriptBlock[] = [
  {
    id: 1,
    kind: 'user',
    text: 'The retry backoff in providers looks like it stampedes on recovery — can you add jitter and verify the tests still pass?',
  },
];
