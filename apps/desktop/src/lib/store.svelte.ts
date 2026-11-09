// SessionStore — the ONLY state hub. Fed by Tauri `core-event` emissions
// (or the fixture script when running outside Tauri). Components read
// state; they never decide agent behavior.

import { untrack } from 'svelte';
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
  TurnOutcome,
} from './types';
import type { ProviderInfo, ProviderModel, SessionMeta, ThinkingLevel } from './ipc';
import { isThemeId } from './themes';
import { copyText, parsePlan, transcriptMarkdown } from './format';

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
export type SessionMode = 'chat' | 'code';

const MODES: readonly AppMode[] = ['home', 'code', 'chat', 'settings'];

let nextBlockId = 2;

function fmtK(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

/** The frontend-owned half of a persisted session — serialized to the
 *  `ui_state` column. The backend owns message history separately. */
interface UiState {
  blocks: TranscriptBlock[];
  draft: string;
  plan: PlanStep[];
  usage: TokenUsage;
  sessionTurns: number;
  sessionBeganAt: number | null;
  sessionTotals: TokenUsage;
  ctxUsed: number;
  costUsd: number;
  cacheRatio: number;
  model: string;
  provider: string;
  providerName: string;
  thinkingLevel: ThinkingLevel;
  checkpoints: CheckpointEntry[];
  changedFiles: ChangedFile[];
  planMode: boolean;
}

/** Sent when the user approves a plan — the plan itself is already in
 *  the conversation, so this stays short. */
const RUN_PLAN_TEXT =
  'Approved. Carry out the plan above step by step, then verify the result.';

class SessionStore {
  // ── transcript ──────────────────────────────────────────────────────────
  // Empty in the real app — fixtures only load for browser dev playback.
  blocks = $state<TranscriptBlock[]>([]);

  // ── left pane ───────────────────────────────────────────────────────────
  tree = $state<FileNode[]>([]);
  symbols = $state<SymbolHit[]>([]);
  indexTruncated = $state(false);
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
  /** Messages typed while a turn was running — sent in order, one per
   *  completed turn. */
  queue = $state<string[]>([]);

  // ── plan mode ───────────────────────────────────────────────────────────
  /** While on, every send is a read-only `/plan` turn. */
  planMode = $state(false);
  /** A plan turn finished with steps — the plan bar offers Run/Revise. */
  planReady = $state(false);

  // ── panes ───────────────────────────────────────────────────────────────
  leftCollapsed = $state(false);
  rightCollapsed = $state(false);
  leftWidth = $state(260);
  rightWidth = $state(300);

  // ── sessions ────────────────────────────────────────────────────────────
  /** Active persisted session id; `null` = none opened yet (ephemeral). */
  sessionId = $state<string | null>(null);
  /** Metadata for the active session (title/model/mode for the topbar). */
  sessionMeta = $state<SessionMeta | null>(null);
  /** Rows for the session browser, filtered to `pickerMode`. */
  sessions = $state<SessionMeta[]>([]);
  sessionsState = $state<'idle' | 'loading' | 'ready' | 'error'>('idle');
  /** Non-null → the view shows the session browser instead of a chat. */
  pickerMode = $state<SessionMode | null>(null);
  /** An open/new/delete IPC is in flight. */
  sessionBusy = $state(false);
  /** Configured providers — drives the empty-state connect hint. */
  providers = $state<ProviderInfo[]>([]);

  // ── app shell ───────────────────────────────────────────────────────────
  /** Landing picks the surface; persisted across launches. */
  mode = $state<AppMode>('home');
  /** `auto` follows the OS; explicit themes force the palette.
   *  Ids come from `themes.ts` — app.css defines each `data-theme`. */
  theme = $state<string>('auto');
  /** Folder the code session operates in (from config.toml). */
  workspacePath = $state<string | null>(null);

  // ── internals ───────────────────────────────────────────────────────────
  private unlisten: UnlistenFn | null = null;
  private fixtureTimers: ReturnType<typeof setTimeout>[] = [];
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  /** Block index where the current provider request's output begins —
   *  `tokens_counted` attributes usage to blocks from here on. */
  private roundStart = 0;
  /** Usage of a request that produced no text or thinking; lands on the
   *  first tool card it started. */
  private pendingToolUsage: TokenUsage | null = null;
  /** The in-flight turn's user block, for the turn rollup. */
  private turnBlockId: number | null = null;
  private turnIsPlan = false;
  /** The in-flight turn is carrying out an approved plan. */
  private planExecuting = false;

  get running(): boolean {
    return this.sessionState === 'running';
  }

  /** Catalog row for the active model (undefined until models load). */
  get selectedModel(): ProviderModel | undefined {
    // Provider-aware: two providers can list the same id (e.g. an
    // aggregator mirroring a vendor catalog).
    return (
      this.models.find((m) => m.id === this.model && m.provider === this.providerName) ??
      this.models.find((m) => m.id === this.model)
    );
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
    if (isThemeId(savedTheme)) this.theme = savedTheme;
    const savedThinking = localStorage.getItem('hate:thinking');
    if (isThinkingLevel(savedThinking)) this.thinkingLevel = savedThinking;
    this.applyTheme();
    if (inTauri()) {
      const m = await import('./ipc');
      this.workspacePath = await m.workspaceGet().catch(() => null);
      void this.refreshIndex();
      // Push the persisted thinking level into the session slot so the
      // first spawned session inherits it (live swaps go through
      // `session_set_options` too).
      void m
        .sessionSetOptions(
          null,
          this.thinkingLevel,
          this.selectedModel?.context_tokens ?? null,
        )
        .catch(() => {});
      this.providers = await m.providersList().catch(() => []);
      const def = this.providers.find((p) => p.is_default) ?? this.providers[0];
      if (def) {
        this.providerName = def.name;
        this.provider = def.name;
        this.model = def.model;
      }
      void this.refreshModels();
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
      // Browser dev mode only: lets layout tests and manual poking feed
      // arbitrary events through the real pipeline. Never set in Tauri.
      (window as unknown as { __hateSession?: SessionStore }).__hateSession = this;
    }
  }

  destroy(): void {
    this.unlisten?.();
    this.fixtureTimers.forEach(clearTimeout);
    this.fixtureTimers = [];
  }

  // ── commands ────────────────────────────────────────────────────────────

  /** Send the composer draft. In Tauri this becomes a `session_send`
   *  invoke; the first send auto-creates the persisted session. */
  send(override?: string): void {
    const text = (override ?? this.draft).trim();
    if (!text) return;
    if (override === undefined) this.draft = '';
    // Busy → queue instead of dropping; drained as turns complete.
    if (this.sessionState === 'running' || this.sessionState === 'awaiting_permission') {
      this.queue.push(text);
      return;
    }
    const plan = this.planMode;
    // Core recognises the `/plan ` prefix and runs a read-only turn;
    // the transcript keeps the text as the user typed it.
    const wire = plan ? `/plan ${text}` : text;
    this.planReady = false;
    const begin = () => {
      const id = nextBlockId++;
      this.blocks.push(plan ? { id, kind: 'user', text, plan: true } : { id, kind: 'user', text });
      this.turnBlockId = id;
      this.turnIsPlan = plan;
      this.roundStart = this.blocks.length;
      this.pendingToolUsage = null;
      this.sessionState = 'running';
      this.sessionTurns += 1;
      this.sessionBeganAt ??= Date.now();
      this.finishStreamingBlocks();
      this.scheduleUiSave();

      if (!inTauri()) {
        // Outside Tauri there is no backend — leave the turn visibly
        // "running" so the UI demonstrates the busy state honestly.
        return;
      }
      void import('./ipc')
        .then((m) => m.sessionSend(wire))
        .catch((err: unknown) => {
          this.sessionState = 'failed';
          this.pushError(String(err));
          this.scheduleUiSave();
        });
    };
    if (inTauri() && !this.sessionId) {
      void this.ensureSession()
        .then(begin)
        .catch((err: unknown) => {
          this.draft = text;
          this.pushError(`session error: ${String(err)}`);
        });
    } else {
      begin();
    }
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

  // ── plan mode ───────────────────────────────────────────────────────────

  /** `/plan` — toggle plan mode. Leaving it dismisses a pending plan bar
   *  but keeps the steps in the inspector for reference. */
  togglePlan(): void {
    this.planMode = !this.planMode;
    if (!this.planMode) this.planReady = false;
    this.scheduleUiSave();
  }

  /** Approve the pending plan: leave plan mode and run it. */
  runPlan(): void {
    if (!this.planReady || this.running) return;
    this.planMode = false;
    this.planReady = false;
    this.planExecuting = true;
    this.plan = this.plan.map((s, i) => ({ ...s, status: i === 0 ? 'active' : 'pending' }));
    this.send(RUN_PLAN_TEXT);
  }

  /** Keep planning — hide the bar; the next message revises the plan. */
  revisePlan(): void {
    this.planReady = false;
  }

  /** Drop the plan and leave plan mode. */
  discardPlan(): void {
    this.planReady = false;
    this.planMode = false;
    this.plan = [];
    this.scheduleUiSave();
  }

  // ── transcript actions ──────────────────────────────────────────────────

  removeQueued(index: number): void {
    this.queue.splice(index, 1);
  }

  /** Text of the most recent user message (composer ↑ recall, `/retry`). */
  lastUserText(): string | null {
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      const b = this.blocks[i];
      if (b?.kind === 'user' && b.text) return b.text;
    }
    return null;
  }

  private lastUserBlockId(): number | null {
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      const b = this.blocks[i];
      if (b?.kind === 'user') return b.id;
    }
    return null;
  }

  /** `/retry` — send the last user message again. */
  retryLast(): void {
    const text = this.lastUserText();
    if (text) this.send(text);
    else this.note('nothing to retry yet');
  }

  /** `/copy` — copy the latest assistant reply. */
  async copyLastReply(): Promise<void> {
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      const b = this.blocks[i];
      if (b?.kind === 'text' && !b.local && b.text) {
        const ok = await copyText(b.text);
        this.note(ok ? 'copied the last reply' : 'clipboard unavailable');
        return;
      }
    }
    this.note('no reply to copy yet');
  }

  /** `/export` — copy the whole transcript as Markdown. */
  async exportTranscript(): Promise<void> {
    if (this.blocks.length === 0) {
      this.note('nothing to export yet');
      return;
    }
    const ok = await copyText(transcriptMarkdown(this.blocks));
    this.note(ok ? 'transcript copied to the clipboard as Markdown' : 'clipboard unavailable');
  }

  /** A local, non-model line in the transcript (command feedback). */
  note(text: string): void {
    this.blocks.push({ id: nextBlockId++, kind: 'text', text, streaming: false, local: true });
  }

  /** Rewind the workspace to a checkpoint's pre-turn file state.
   *  Leaves a transcript note so the rewind is visible in the record. */
  async rewindCheckpoint(id: string): Promise<void> {
    if (!inTauri()) return;
    try {
      const { sessionRewind } = await import('./ipc');
      await sessionRewind(id);
      this.blocks.push({
        id: nextBlockId++,
        kind: 'text',
        text: `⏪ workspace rewound to checkpoint ${id.slice(0, 8)}`,
        streaming: false,
      });
      void this.refreshIndex();
    } catch (e) {
      this.blocks.push({
        id: nextBlockId++,
        kind: 'error',
        text: `rewind failed: ${e instanceof Error ? e.message : String(e)}`,
      });
    }
  }

  /** Reload the cross-provider model catalog. Called at startup and
   *  after a provider is connected/disconnected/added so the picker
   *  never goes stale mid-session. */
  async refreshModels(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.providers = await m.providersList().catch(() => []);
    this.modelsState = 'loading';
    // Fan out across every configured provider so the picker can
    // group by provider; unauthed/unreachable ones just yield [].
    const lists = await Promise.all(
      this.providers.map((p) =>
        m
          .modelsList(p.name)
          .then((list) => list.map((x) => ({ ...x, provider: p.name })))
          .catch(() => [] as ProviderModel[]),
      ),
    );
    try {
      const list = lists.length > 0 ? lists.flat() : await m.modelsList();
      this.models = filterChatModels(list);
      this.modelsState = 'ready';
    } catch (err) {
      this.modelsState = 'error';
      this.modelsError = String(err);
    }
  }

  /** Pick a model — persists to config.toml (CLI shares it) and
   *  hot-swaps the live session. `provider` tags catalog rows from the
   *  multi-provider fan-out; a pick from a *different* provider swaps
   *  the session's adapter too (adapters are stateless), so it applies
   *  from the next turn with no respawn.
   *
   *  Transactional: the UI's provider/model only change once the
   *  backend has accepted the swap. If the provider can't be reached
   *  (logged out, removed), the pick is refused with a visible note and
   *  nothing moves — UI and session can never disagree. */
  async setModel(id: string, provider?: string | null): Promise<void> {
    const target = provider || this.providerName;
    if (!id || (id === this.model && target === this.providerName)) return;
    if (!inTauri()) {
      this.model = id;
      if (target) this.providerName = this.provider = target;
      return;
    }
    const m = await import('./ipc');
    const crossProvider = !!target && target !== this.providerName;
    if (crossProvider) {
      try {
        // Adapter + model swap as ONE backend operation — a model id
        // must never ride into a session running another provider.
        await m.sessionSetProvider(target, id);
      } catch (e) {
        this.blocks.push({
          id: nextBlockId++,
          kind: 'error',
          text: `couldn't switch to ${target}: ${e instanceof Error ? e.message : String(e)} — still on ${this.providerName}`,
        });
        return;
      }
      this.providerName = target;
      this.provider = target;
      // The backend also made `target` the default provider.
      this.providers = this.providers.map((p) => ({ ...p, is_default: p.name === target }));
    }
    this.model = id;
    await m.providerSetModel(target, id).catch(() => {});
    await m
      .sessionSetOptions(id, null, this.selectedModel?.context_tokens ?? null)
      .catch(() => {});
    if (crossProvider) {
      this.blocks.push({
        id: nextBlockId++,
        kind: 'text',
        text: `switched to ${target} · ${this.selectedModel?.name ?? id}`,
        streaming: false,
      });
    }
    this.scheduleUiSave();
  }

  /** Push the UI's provider + model to the backend slot as one unit —
   *  used whenever a session is primed (new/opened) so the next spawn
   *  runs exactly the pair the composer shows. If the provider is
   *  unusable, fall back to the configured default rather than leave
   *  the UI showing a pair the backend won't run. */
  private async syncSelection(m: typeof import('./ipc')): Promise<void> {
    if (this.providerName) {
      try {
        await m.sessionSetProvider(this.providerName, this.model || null);
      } catch (e) {
        const def = this.providers.find((p) => p.is_default) ?? this.providers[0];
        this.blocks.push({
          id: nextBlockId++,
          kind: 'error',
          text: `${this.providerName} unavailable: ${e instanceof Error ? e.message : String(e)}${def ? ` — using ${def.name}` : ''}`,
        });
        if (def) {
          this.providerName = this.provider = def.name;
          this.model = def.model;
        }
      }
    }
    await m
      .sessionSetOptions(
        this.model || null,
        this.thinkingLevel,
        this.selectedModel?.context_tokens ?? null,
      )
      .catch(() => {});
  }

  /** Pick a thinking level — applies next turn, persisted locally. */
  setThinking(level: ThinkingLevel): void {
    if (level === this.thinkingLevel) return;
    this.thinkingLevel = level;
    localStorage.setItem('hate:thinking', level);
    if (inTauri()) {
      void import('./ipc')
        .then((m) =>
          m.sessionSetOptions(
            null,
            level,
            this.selectedModel?.context_tokens ?? null,
          ),
        )
        .catch(() => {});
    }
    this.scheduleUiSave();
  }

  cycleThinking(): void {
    const order: ThinkingLevel[] = ['off', 'low', 'medium', 'high'];
    const i = order.indexOf(this.thinkingLevel);
    this.setThinking(order[(i + 1) % order.length] ?? 'off');
  }

  /** `/model` — cycle the provider catalog; persists + hot-swaps the
   *  live session exactly like picking from the model menu. */
  async cycleModel(): Promise<void> {
    const list = this.models;
    if (list.length === 0) return;
    const i = list.findIndex(
      (m) => m.id === this.model && (!m.provider || m.provider === this.providerName),
    );
    const next = list[(i + 1) % list.length];
    if (next) await this.setModel(next.id, next.provider);
  }

  /** `/compact` — ask the backend to shrink context now; the
   *  compaction event lands in the transcript when it finishes. */
  async compact(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.sessionCompact().catch(() => {});
  }

  // ── session lifecycle ───────────────────────────────────────────────────

  /** Which persisted-session mode the current surface maps to. */
  sessionMode(): SessionMode {
    return this.mode === 'code' ? 'code' : 'chat';
  }

  /** Entering Chat/Code resumes that mode's last session, or opens the
   *  browser when none exists. Called from App's mode effect. */
  async enterMode(mode: SessionMode): Promise<void> {
    if (this.pickerMode === mode) {
      void this.refreshSessions(mode);
      return;
    }
    if (this.sessionId && this.sessionMeta?.mode === mode) {
      this.pickerMode = null;
      return;
    }
    const saved = inTauri() ? localStorage.getItem(`hate:session:${mode}`) : null;
    if (saved) {
      try {
        await this.openSession(saved);
        return;
      } catch {
        localStorage.removeItem(`hate:session:${mode}`);
      }
    }
    this.pickerMode = mode;
    void this.refreshSessions(mode);
  }

  async refreshSessions(mode?: SessionMode): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.sessionsState = 'loading';
    try {
      this.sessions = await m.sessionsList(mode ?? this.pickerMode ?? undefined);
      this.sessionsState = 'ready';
    } catch {
      this.sessionsState = 'error';
    }
  }

  /** Open a persisted session — restores transcript UI and primes the
   *  backend actor (message history reloads on next send). */
  async openSession(id: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.sessionBusy = true;
    try {
      const res = await m.sessionOpen(id);
      this.sessionId = res.meta.id;
      this.sessionMeta = res.meta;
      this.applyUiState(res.ui_state);
      // The journal is authoritative for provider/model — adopt it so
      // the composer never shows a pair the session isn't running.
      if (res.meta.provider) {
        this.providerName = res.meta.provider;
        this.provider = res.meta.provider;
      }
      if (res.meta.model) this.model = res.meta.model;
      // A code session owns its folder — adopt it so the file tree and
      // future turns agree with what the session was created against.
      if (
        res.meta.mode === 'code' &&
        res.meta.workspace &&
        res.meta.workspace !== this.workspacePath
      ) {
        this.workspacePath = res.meta.workspace;
        void m.workspaceSet(res.meta.workspace).catch(() => {});
        void this.refreshIndex();
      }
      // Hand the session's provider + model + thinking to the backend
      // slot as one unit so the next spawn runs what the UI shows.
      await this.syncSelection(m);
      localStorage.setItem(`hate:session:${res.meta.mode}`, id);
      this.pickerMode = null;
      void this.refreshSessions(res.meta.mode as SessionMode);
    } finally {
      this.sessionBusy = false;
    }
  }

  /** Start a fresh persisted session in the current mode. */
  async newSession(): Promise<void> {
    if (!inTauri()) {
      this.resetConversation();
      this.pickerMode = null;
      return;
    }
    const m = await import('./ipc');
    this.sessionBusy = true;
    try {
      const meta = await m.sessionNew(this.sessionMode());
      this.sessionId = meta.id;
      this.sessionMeta = meta;
      this.resetConversation();
      // A fresh slot falls back to the default provider — pin it to the
      // pair the composer shows instead.
      await this.syncSelection(m);
      localStorage.setItem(`hate:session:${meta.mode}`, meta.id);
      this.pickerMode = null;
      void this.refreshSessions(meta.mode);
    } finally {
      this.sessionBusy = false;
    }
  }

  /** Permanently delete a session; if it's the active one, fall back
   *  to the browser for that mode. */
  async deleteSession(id: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.sessionDelete(id);
    this.sessions = this.sessions.filter((s) => s.id !== id);
    if (this.sessionId === id) {
      this.sessionId = null;
      this.sessionMeta = null;
      this.resetConversation();
      this.pickerMode = this.sessionMode();
      void this.refreshSessions(this.pickerMode);
    }
  }

  /** Wipe conversation state without touching persistence — used by
   *  `newSession` and session restore. */
  private resetConversation(): void {
    this.blocks = [];
    nextBlockId = 1;
    this.plan = [];
    this.usage = { input: 0, cached_input: 0, output: 0, thinking: 0 };
    this.checkpoints = [];
    this.changedFiles = [];
    this.sessionTurns = 0;
    this.sessionBeganAt = null;
    this.sessionTotals = { input: 0, cached_input: 0, output: 0, thinking: 0 };
    this.ctxUsed = 0;
    this.costUsd = 0;
    this.cacheRatio = 0;
    this.draft = '';
    this.pendingPermission = null;
    this.awaitingPrompt = null;
    this.sessionState = 'idle';
    this.queue = [];
    this.planMode = false;
    this.planReady = false;
    this.planExecuting = false;
    this.turnBlockId = null;
    this.pendingToolUsage = null;
    this.roundStart = 0;
  }

  /** Restore the frontend half of a session from its `ui_state` blob. */
  private applyUiState(raw: string | null): void {
    if (!raw) {
      this.resetConversation();
      return;
    }
    try {
      const s = JSON.parse(raw) as Partial<UiState>;
      this.blocks = s.blocks ?? [];
      nextBlockId = this.blocks.reduce((m, b) => Math.max(m, b.id + 1), 1);
      this.draft = s.draft ?? '';
      this.plan = s.plan ?? [];
      this.usage = s.usage ?? { input: 0, cached_input: 0, output: 0, thinking: 0 };
      this.sessionTurns = s.sessionTurns ?? 0;
      this.sessionBeganAt = s.sessionBeganAt ?? null;
      this.sessionTotals =
        s.sessionTotals ?? { input: 0, cached_input: 0, output: 0, thinking: 0 };
      this.ctxUsed = s.ctxUsed ?? 0;
      this.costUsd = s.costUsd ?? 0;
      this.cacheRatio = s.cacheRatio ?? 0;
      this.model = s.model ?? this.model;
      this.provider = s.provider ?? this.provider;
      this.providerName = s.providerName ?? this.providerName;
      if (isThinkingLevel(s.thinkingLevel ?? null)) {
        this.thinkingLevel = s.thinkingLevel as ThinkingLevel;
      }
      this.checkpoints = s.checkpoints ?? [];
      this.changedFiles = s.changedFiles ?? [];
      this.planMode = s.planMode ?? false;
      this.planReady = false;
      this.planExecuting = false;
      this.queue = [];
      this.turnBlockId = null;
      this.pendingToolUsage = null;
      this.roundStart = this.blocks.length;
      this.pendingPermission = null;
      this.awaitingPrompt = null;
      this.sessionState = 'idle';
      this.finishStreamingBlocks();
    } catch {
      this.resetConversation();
    }
  }

  private uiSnapshot(): string {
    const s: UiState = {
      blocks: this.blocks,
      draft: this.draft,
      plan: this.plan,
      usage: this.usage,
      sessionTurns: this.sessionTurns,
      sessionBeganAt: this.sessionBeganAt,
      sessionTotals: this.sessionTotals,
      ctxUsed: this.ctxUsed,
      costUsd: this.costUsd,
      cacheRatio: this.cacheRatio,
      model: this.model,
      provider: this.provider,
      providerName: this.providerName,
      thinkingLevel: this.thinkingLevel,
      checkpoints: this.checkpoints,
      changedFiles: this.changedFiles,
      planMode: this.planMode,
    };
    return JSON.stringify(s);
  }

  /** Debounced ui_state write — every transcript mutation schedules one;
   *  the last write within ~800ms wins. */
  private scheduleUiSave(): void {
    if (!inTauri() || !this.sessionId) return;
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      const id = this.sessionId;
      if (!id) return;
      const blob = this.uiSnapshot();
      void import('./ipc')
        .then((m) => m.sessionSaveUi(id, blob))
        .catch(() => {});
    }, 800);
  }

  /** Guarantee an active persisted session before a send; falls back to
   *  an ephemeral session when persistence is unavailable. */
  private async ensureSession(): Promise<void> {
    if (this.sessionId || !inTauri()) return;
    await this.newSession();
  }

  clearTranscript(): void {
    this.blocks = [];
    nextBlockId = 1;
    this.sessionTurns = 0;
    this.sessionBeganAt = null;
    this.sessionTotals = { input: 0, cached_input: 0, output: 0, thinking: 0 };
    this.ctxUsed = 0;
    this.roundStart = 0;
    this.pendingToolUsage = null;
    this.turnBlockId = null;
    this.planReady = false;
  }

  setMode(mode: AppMode): void {
    this.mode = mode;
    localStorage.setItem('hate:mode', mode);
  }

  setTheme(theme: string): void {
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
      void this.refreshIndex();
    }
  }

  /** Rebuild the file tree + symbol index for the current workspace.
   *  Called on workspace change, session open, and after each turn
   *  (file tools may have mutated the tree). Keeps the stale tree on
   *  failure rather than blanking the pane. */
  async refreshIndex(): Promise<void> {
    const root = this.workspacePath;
    if (!inTauri() || !root) {
      this.tree = [];
      this.symbols = [];
      this.indexTruncated = false;
      return;
    }
    const m = await import('./ipc');
    try {
      const idx = await m.workspaceIndex(root);
      // A workspace switch mid-walk must not clobber the new tree.
      if (this.workspacePath !== root) return;
      this.tree = idx.tree;
      this.symbols = idx.symbols;
      this.indexTruncated = idx.truncated;
    } catch {
      // stale tree beats an empty pane
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
          ...(this.pendingToolUsage ? { usage: this.pendingToolUsage } : {}),
        });
        this.pendingToolUsage = null;
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
        this.attributeUsage(e.usage, e.thinking_est ?? 0);
        break;
      case 'compaction_started':
        // Context was compacted — say so in the record; the model's
        // memory shrank even though the transcript didn't.
        this.blocks.push({
          id: nextBlockId++,
          kind: 'text',
          text: `🗜 context compacted — ${fmtK(e.before)} → ${fmtK(e.after)} tok (${e.reason})`,
          streaming: false,
          local: true,
        });
        this.ctxUsed = e.after;
        break;
      case 'checkpoint_created':
        this.checkpoints = [
          { id: e.id, label: 'checkpoint', at: new Date().toLocaleTimeString() },
          ...this.checkpoints,
        ];
        break;
      case 'skill_saved':
        // The agent authored a reusable skill — visible, never silent.
        this.blocks.push({
          id: nextBlockId++,
          kind: 'text',
          text: `📚 learned skill: ${e.title} — ${(e.bytes / 1024).toFixed(1)} KB (Settings → Skills)`,
          streaming: false,
          local: true,
        });
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
        void this.refreshIndex();
        this.finishTurn(e.outcome);
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
        if (this.planExecuting) {
          // Whatever wasn't reached stays visibly unfinished.
          this.plan = this.plan.map((s) =>
            s.status === 'done' ? s : { ...s, status: 'failed' },
          );
          this.planExecuting = false;
        }
        break;
      case 'error':
        this.pushError(e.error.message ?? 'unknown error');
        break;
    }
    // Every event can mutate transcript state — debounce-persists the
    // UI blob so a crash loses at most ~1s of chrome (message history
    // is journaled by the actor at turn end regardless).
    this.scheduleUiSave();
  }

  /** Pin one provider request's usage to what it produced: its reply
   *  text, else its thinking, else (tool-only request) the first tool
   *  card it starts. Thinking blocks get the reasoning share — exact
   *  when the provider reports it, a local count otherwise. */
  private attributeUsage(usage: TokenUsage, thinkingEst: number): void {
    let textBlock: TranscriptBlock | undefined;
    let thinkBlock: TranscriptBlock | undefined;
    for (let i = this.blocks.length - 1; i >= this.roundStart; i--) {
      const b = this.blocks[i];
      if (!b || b.usage) continue;
      if (b.kind === 'text' && b.streaming && !textBlock) textBlock = b;
      if (b.kind === 'thinking' && !thinkBlock) thinkBlock = b;
    }
    if (thinkBlock) {
      const exact = usage.thinking > 0;
      const n = exact ? usage.thinking : thinkingEst;
      if (n > 0) {
        thinkBlock.thinkingTokens = n;
        thinkBlock.thinkingEstimated = !exact;
      }
    }
    const owner = textBlock ?? thinkBlock;
    if (owner) owner.usage = { ...usage };
    else this.pendingToolUsage = { ...usage };
    // The request is over: its blocks stop streaming now, so the caret
    // goes and the usage shows while the tools it called are running.
    if (textBlock) textBlock.streaming = false;
    if (thinkBlock) thinkBlock.streaming = false;
    this.roundStart = this.blocks.length;
  }

  /** Turn-end bookkeeping: rollup on the user's message, plan capture,
   *  and the next queued message. */
  private finishTurn(outcome: TurnOutcome): void {
    const idx = this.blocks.findIndex((b) => b.id === this.turnBlockId);
    const userBlock = idx >= 0 ? this.blocks[idx] : undefined;
    const completed = outcome.outcome === 'Completed';
    if (userBlock && completed) {
      userBlock.turn = {
        usage: { ...outcome.usage },
        toolCalls: outcome.tool_calls,
        elapsedMs: outcome.elapsed_ms,
        costUsd: outcome.cost_usd,
      };
    }
    if (this.turnIsPlan && completed && idx >= 0) {
      // The plan is the turn's final reply.
      for (let i = this.blocks.length - 1; i > idx; i--) {
        const b = this.blocks[i];
        if (b?.kind === 'text' && b.text) {
          const steps = parsePlan(b.text);
          if (steps.length > 0) {
            this.plan = steps;
            this.planReady = true;
          }
          break;
        }
      }
    }
    if (this.planExecuting) {
      if (completed) this.plan = this.plan.map((s) => ({ ...s, status: 'done' }));
      this.planExecuting = false;
    }
    this.turnBlockId = null;
    this.turnIsPlan = false;
    // A pending plan needs the user's decision first; a cancelled turn
    // should not be followed by more automatic sends.
    if (completed && !this.planReady && this.queue.length > 0) {
      const next = this.queue.shift();
      if (next) queueMicrotask(() => this.send(next));
    }
  }

  private recomputeDerived(): void {
    // Session-cumulative cache hit ratio: cached input over ALL input
    // across completed turns. Per-round ratios fluctuate wildly during
    // tool loops; the honest first-class metric is hits/session input.
    const t = this.sessionTotals;
    this.cacheRatio = t.input > 0 ? t.cached_input / t.input : 0;
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
    // Treat the canned transcript as one in-flight turn so usage
    // attribution and the turn rollup demo exactly like a live session.
    this.turnBlockId = this.lastUserBlockId();
    this.roundStart = this.blocks.length;
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
