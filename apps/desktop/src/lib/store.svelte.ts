// SessionStore — the ONLY state hub. Fed by Tauri `core-event` emissions
// (or the fixture script when running outside Tauri). Components read
// state; they never decide agent behavior.

import type { UnlistenFn } from '@tauri-apps/api/event';
import {
  fixtureChangedFiles,
  fixtureCheckpoints,
  fixtureInitialBlocks,
  fixturePlan,
  fixtureScript,
  fixtureSymbols,
  fixtureTree,
  fixtureUsage,
} from './fixtures';
import type {
  ChangedFile,
  CheckpointEntry,
  FailureReason,
  FileNode,
  PermissionRequest,
  PlanStep,
  SessionEvent,
  SymbolHit,
  TokenUsage,
  TranscriptBlock,
} from './types';
import type { ProviderModel, ThinkingLevel } from './ipc';

const inTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const THINKING_LEVELS: readonly ThinkingLevel[] = ['off', 'low', 'medium', 'high'];

function isThinkingLevel(v: string | null): v is ThinkingLevel {
  return v !== null && (THINKING_LEVELS as readonly string[]).includes(v);
}

/** Non-chat entries (embeddings, rerankers, batch SKUs) don't belong in
 *  the composer picker — the catalog is a full router listing. */
function filterChatModels(models: ProviderModel[]): ProviderModel[] {
  const SKIP = /embed|bge-|e5-|gte-|voyage|paraphrase|rerank|:batch/i;
  return models
    .filter((m) => !SKIP.test(m.id))
    .sort((a, b) => a.id.localeCompare(b.id));
}

/** Human-readable text for a serde externally-tagged `FailureReason`. */
function failureText(r: FailureReason): string {
  if ('Provider' in r) return r.Provider.message;
  if ('LimitExceeded' in r) return `limit exceeded: ${r.LimitExceeded.limit}`;
  if ('Tool' in r) return `tool ${r.Tool.name}: ${r.Tool.message}`;
  if ('PermissionDenied' in r) return `permission denied: ${r.PermissionDenied.tool}`;
  if ('Internal' in r) return r.Internal.message;
  return 'unknown failure';
}

export type AppMode = 'home' | 'code' | 'chat' | 'settings';

const MODES: readonly AppMode[] = ['home', 'code', 'chat', 'settings'];

let nextBlockId = 2;

class SessionStore {
  // ── transcript ──────────────────────────────────────────────────────────
  // Empty in the real app — fixtures only load for browser dev playback.
  blocks = $state<TranscriptBlock[]>([]);

  // ── left pane ───────────────────────────────────────────────────────────
  tree = $state<FileNode[]>([]);
  symbols = $state<SymbolHit[]>([]);
  selectedPath = $state<string | null>(null);

  // ── right pane ──────────────────────────────────────────────────────────
  plan = $state<PlanStep[]>([]);
  usage = $state<TokenUsage>({ input: 0, cached_input: 0, output: 0, thinking: 0 });
  budgetTokens = 200_000;
  checkpoints = $state<CheckpointEntry[]>([]);
  changedFiles = $state<ChangedFile[]>([]);
  model = $state('');
  provider = $state('');
  /** Config name of the active provider (`nous`, …) — for `provider_set_model`. */
  providerName = $state('');
  sessionState = $state<'idle' | 'running' | 'awaiting_permission' | 'awaiting_user' | 'done' | 'failed'>(
    'idle',
  );
  costUsd = $state(0);
  cacheRatio = $state(0);

  // ── model picker / thinking ─────────────────────────────────────────────
  models = $state<ProviderModel[]>([]);
  modelsState = $state<'idle' | 'loading' | 'ready' | 'error'>('idle');
  modelsError = $state('');
  thinkingLevel = $state<ThinkingLevel>('off');

  // ── session stats (Claude-Code-style status line) ───────────────────────
  /** User messages sent this session — the turn counter. */
  sessionTurns = $state(0);
  /** First-send timestamp; `null` before the session starts. */
  sessionBeganAt = $state<number | null>(null);
  /** Cumulative token spend across completed turns. */
  sessionTotals = $state<TokenUsage>({ input: 0, cached_input: 0, output: 0, thinking: 0 });
  /** Prompt tokens of the most recent request — the live context fill. */
  ctxUsed = $state(0);

  // ── composer / permission ───────────────────────────────────────────────
  draft = $state('');
  pendingPermission = $state<PermissionRequest | null>(null);
  awaitingPrompt = $state<string | null>(null);

  // ── panes ───────────────────────────────────────────────────────────────
  leftCollapsed = $state(false);
  rightCollapsed = $state(false);
  leftWidth = $state(260);
  rightWidth = $state(300);

  // ── app shell ───────────────────────────────────────────────────────────
  /** Landing picks the surface; persisted across launches. */
  mode = $state<AppMode>('home');
  /** `auto` follows the OS; explicit themes force the palette. */
  theme = $state<'auto' | 'dark' | 'light'>('auto');
  /** Folder the code session operates in (from config.toml). */
  workspacePath = $state<string | null>(null);

  // ── internals ───────────────────────────────────────────────────────────
  private unlisten: UnlistenFn | null = null;
  private fixtureTimers: ReturnType<typeof setTimeout>[] = [];

  get running(): boolean {
    return this.sessionState === 'running';
  }

  /** Catalog row for the active model (undefined until models load). */
  get selectedModel(): ProviderModel | undefined {
    return this.models.find((m) => m.id === this.model);
  }

  /** Context window to measure `ctxUsed` against — real catalog value
   *  when known, else the conservative default. */
  get ctxLimit(): number {
    return this.selectedModel?.context_tokens ?? this.budgetTokens;
  }

  /** 0..1 fill of the current context window. */
  get ctxRatio(): number {
    return this.ctxLimit > 0 ? Math.min(1, this.ctxUsed / this.ctxLimit) : 0;
  }

  /** Wire the event stream. Called once on app mount. */
  async start(): Promise<void> {
    const saved = localStorage.getItem('hate:mode');
    if (MODES.includes(saved as AppMode)) this.mode = saved as AppMode;
    const savedTheme = localStorage.getItem('hate:theme');
    if (savedTheme === 'dark' || savedTheme === 'light') this.theme = savedTheme;
    const savedThinking = localStorage.getItem('hate:thinking');
    if (isThinkingLevel(savedThinking)) this.thinkingLevel = savedThinking;
    this.applyTheme();
    if (inTauri()) {
      const m = await import('./ipc');
      this.workspacePath = await m.workspaceGet().catch(() => null);
      // Push the persisted thinking level into the session slot so the
      // first spawned session inherits it (live swaps go through
      // `session_set_options` too).
      void m.sessionSetOptions(null, this.thinkingLevel).catch(() => {});
      const providers = await m.providersList().catch(() => []);
      const def = providers.find((p) => p.is_default) ?? providers[0];
      if (def) {
        this.providerName = def.name;
        this.provider = def.name;
        this.model = def.model;
      }
      this.modelsState = 'loading';
      m.modelsList()
        .then((list) => {
          this.models = filterChatModels(list);
          this.modelsState = 'ready';
        })
        .catch((err: unknown) => {
          this.modelsState = 'error';
          this.modelsError = String(err);
        });
      const { listen } = await import('@tauri-apps/api/event');
      this.unlisten = await listen<SessionEvent>('core-event', (e) => {
        this.handleEvent(e.payload);
      });
    } else {
      // Browser dev mode: replay the fixture script so the shell demos
      // itself without a backend.
      this.blocks = [...fixtureInitialBlocks];
      this.tree = fixtureTree;
      this.symbols = fixtureSymbols;
      this.plan = [...fixturePlan];
      this.usage = { ...fixtureUsage };
      this.checkpoints = [...fixtureCheckpoints];
      this.changedFiles = [...fixtureChangedFiles];
      this.model = 'nous/hermes-4-405b';
      this.provider = 'nous';
      this.costUsd = 0.0031;
      this.cacheRatio = 0.82;
      this.startFixturePlayback();
    }
  }

  destroy(): void {
    this.unlisten?.();
    this.fixtureTimers.forEach(clearTimeout);
    this.fixtureTimers = [];
  }

  // ── commands ────────────────────────────────────────────────────────────

  /** Send the composer draft. In Tauri this becomes a `session_send` invoke. */
  send(): void {
    const text = this.draft.trim();
    if (!text || this.running) return;
    this.draft = '';
    this.blocks.push({ id: nextBlockId++, kind: 'user', text });
    this.sessionState = 'running';
    this.sessionTurns += 1;
    this.sessionBeganAt ??= Date.now();
    this.finishStreamingBlocks();

    if (!inTauri()) {
      // Outside Tauri there is no backend — leave the turn visibly "running"
      // so the UI demonstrates the busy state honestly.
      return;
    }
    void import('./ipc')
      .then((m) => m.sessionSend(text))
      .catch((err: unknown) => {
        this.sessionState = 'failed';
        this.pushError(String(err));
      });
  }

  cancel(): void {
    if (!this.running) return;
    this.sessionState = 'idle';
    this.finishStreamingBlocks();
    if (inTauri()) {
      void import('./ipc').then((m) => m.sessionCancel()).catch(() => {});
    }
  }

  respondPermission(decision: 'allow' | 'allow_session' | 'deny'): void {
    const req = this.pendingPermission;
    this.pendingPermission = null;
    if (this.sessionState === 'awaiting_permission') this.sessionState = 'running';
    if (req && inTauri()) {
      void import('./ipc')
        .then((m) => m.sessionRespondPermission(req.id, decision))
        .catch(() => {});
    }
  }

  answerPrompt(text: string): void {
    this.awaitingPrompt = null;
    if (inTauri()) {
      void import('./ipc')
        .then((m) => m.sessionAnswer(text))
        .catch(() => {});
    }
  }

  /** Pick a model from the provider's catalog — persists to config.toml
   *  (CLI shares it) and hot-swaps the live session. */
  async setModel(id: string): Promise<void> {
    if (!id || id === this.model) return;
    this.model = id;
    if (!inTauri()) return;
    const m = await import('./ipc');
    if (this.providerName) {
      await m.providerSetModel(this.providerName, id).catch(() => {});
    }
    await m.sessionSetOptions(id, null).catch(() => {});
  }

  /** Pick a thinking level — applies next turn, persisted locally. */
  setThinking(level: ThinkingLevel): void {
    if (level === this.thinkingLevel) return;
    this.thinkingLevel = level;
    localStorage.setItem('hate:thinking', level);
    if (inTauri()) {
      void import('./ipc')
        .then((m) => m.sessionSetOptions(null, level))
        .catch(() => {});
    }
  }

  cycleThinking(): void {
    const order: ThinkingLevel[] = ['off', 'low', 'medium', 'high'];
    const i = order.indexOf(this.thinkingLevel);
    this.setThinking(order[(i + 1) % order.length] ?? 'off');
  }

  clearTranscript(): void {
    this.blocks = [];
    nextBlockId = 1;
    this.sessionTurns = 0;
    this.sessionBeganAt = null;
    this.sessionTotals = { input: 0, cached_input: 0, output: 0, thinking: 0 };
    this.ctxUsed = 0;
  }

  setMode(mode: AppMode): void {
    this.mode = mode;
    localStorage.setItem('hate:mode', mode);
  }

  setTheme(theme: 'auto' | 'dark' | 'light'): void {
    this.theme = theme;
    localStorage.setItem('hate:theme', theme);
    this.applyTheme();
  }

  private applyTheme(): void {
    if (typeof document === 'undefined') return;
    if (this.theme === 'auto') {
      document.documentElement.removeAttribute('data-theme');
    } else {
      document.documentElement.dataset.theme = this.theme;
    }
  }

  /** Native folder picker → persist to config.toml. */
  async pickWorkspace(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    const p = await m.workspacePick().catch(() => null);
    if (p) {
      this.workspacePath = p;
      await m.workspaceSet(p).catch(() => {});
    }
  }

  // ── event handling ──────────────────────────────────────────────────────

  handleEvent(e: SessionEvent): void {
    switch (e.type) {
      case 'delta':
        this.sessionState = 'running';
        this.appendText(e.text);
        break;
      case 'thinking_delta':
        this.appendThinking(e.text);
        break;
      case 'tool_started':
        this.blocks.push({
          id: nextBlockId++,
          kind: 'tool',
          call: e.call,
          expanded: false,
        });
        break;
      case 'tool_output': {
        const b = this.findTool(e.call_id);
        if (b) b.preview = e.preview;
        break;
      }
      case 'tool_finished': {
        const b = this.findTool(e.call_id);
        if (b) {
          b.ok = e.ok;
          b.fullTokens = e.full_tokens;
          b.sentTokens = e.sent_tokens;
        }
        break;
      }
      case 'permission_requested':
        this.pendingPermission = e.request;
        this.sessionState = 'awaiting_permission';
        break;
      case 'tokens_counted':
        this.usage = { ...e.usage };
        // Per-request prompt size — the live context-window fill.
        this.ctxUsed = e.usage.input;
        this.recomputeDerived();
        break;
      case 'compaction_started':
        // surfaced via inspector toast later; state marker for now
        break;
      case 'checkpoint_created':
        this.checkpoints = [
          { id: e.id, label: 'checkpoint', at: new Date().toLocaleTimeString() },
          ...this.checkpoints,
        ];
        break;
      case 'turn_complete':
        this.usage = { ...e.outcome.usage };
        const t = this.sessionTotals;
        t.input += e.outcome.usage.input;
        t.cached_input += e.outcome.usage.cached_input;
        t.output += e.outcome.usage.output;
        t.thinking += e.outcome.usage.thinking;
        this.costUsd = e.outcome.cost_usd ?? this.costUsd;
        this.recomputeDerived();
        this.sessionState = 'done';
        this.finishStreamingBlocks();
        break;
      case 'awaiting_user':
        this.awaitingPrompt = e.prompt;
        this.sessionState = 'awaiting_user';
        break;
      case 'done':
        this.sessionState = 'done';
        this.finishStreamingBlocks();
        break;
      case 'failed':
        // Terminal failure — surface the reason; a silent dead turn is
        // indistinguishable from "the app does nothing".
        this.pushError(failureText(e.reason));
        this.sessionState = 'failed';
        this.finishStreamingBlocks();
        break;
      case 'error':
        this.pushError(e.error.message ?? 'unknown error');
        break;
    }
  }

  private recomputeDerived(): void {
    const uncached = this.usage.input - this.usage.cached_input;
    this.cacheRatio = this.usage.input > 0 ? this.usage.cached_input / this.usage.input : 0;
    void uncached;
  }

  private findTool(callId: string): TranscriptBlock | undefined {
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      const b = this.blocks[i];
      if (b?.kind === 'tool' && b.call?.id === callId) return b;
    }
    return undefined;
  }

  private appendText(text: string): void {
    const last = this.blocks[this.blocks.length - 1];
    if (last?.kind === 'text' && last.streaming) {
      last.text = (last.text ?? '') + text;
    } else {
      this.blocks.push({ id: nextBlockId++, kind: 'text', text, streaming: true });
    }
  }

  private pushError(text: string): void {
    this.blocks.push({ id: nextBlockId++, kind: 'error', text: `error: ${text}` });
  }

  private appendThinking(text: string): void {
    const last = this.blocks[this.blocks.length - 1];
    if (last?.kind === 'thinking' && last.streaming) {
      last.text = (last.text ?? '') + text;
    } else {
      this.blocks.push({ id: nextBlockId++, kind: 'thinking', text, streaming: true });
    }
  }

  private finishStreamingBlocks(): void {
    for (const b of this.blocks) {
      if (b.kind === 'text' || b.kind === 'thinking') b.streaming = false;
    }
  }

  // ── fixture playback ────────────────────────────────────────────────────

  private startFixturePlayback(): void {
    let t = 600;
    for (const step of fixtureScript) {
      t += step.delay;
      const ev = step.event as SessionEvent;
      this.fixtureTimers.push(
        setTimeout(() => {
          this.handleEvent(ev);
          if (ev.type === 'delta' && !this.running) this.sessionState = 'running';
        }, t),
      );
    }
    // Loop the demo quietly after it finishes so the shell always feels alive.
    this.fixtureTimers.push(
      setTimeout(() => {
        this.sessionState = 'idle';
        this.finishStreamingBlocks();
      }, t + 800),
    );
  }
}

export const session = new SessionStore();
