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
  FileDiff,
  FileNode,
  FileView,
  PermissionRequest,
  PinRef,
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

function filterChatModels(models: ProviderModel[]): ProviderModel[] {
  const SKIP = /embed|bge-|e5-|gte-|voyage|paraphrase|rerank|:batch/i;
  return models
    .filter((m) => !SKIP.test(m.id))
    .sort((a, b) => a.id.localeCompare(b.id));
}

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

const RUN_PLAN_TEXT =
  'Approved. Carry out the plan above step by step, then verify the result.';

export interface ViewerState {
  kind: 'file' | 'diff';
  path: string;
  line: number | null;
  view: FileView | null;
  diff: FileDiff | null;
  loading: boolean;
  error: string | null;
}

export interface QueuedMessage {
  text: string;
  pins: PinRef[];
}

class SessionStore {
  blocks = $state<TranscriptBlock[]>([]);

  tree = $state<FileNode[]>([]);
  symbols = $state<SymbolHit[]>([]);
  indexTruncated = $state(false);
  selectedPath = $state<string | null>(null);
  viewer = $state<ViewerState | null>(null);
  pinned = $state<PinRef[]>([]);

  plan = $state<PlanStep[]>([]);
  usage = $state<TokenUsage>({ input: 0, cached_input: 0, output: 0, thinking: 0 });
  budgetTokens = 200_000;
  checkpoints = $state<CheckpointEntry[]>([]);
  changedFiles = $state<ChangedFile[]>([]);
  model = $state('');
  provider = $state('');
  providerName = $state('');
  sessionState = $state<'idle' | 'running' | 'awaiting_permission' | 'awaiting_user' | 'done' | 'failed'>(
    'idle',
  );
  costUsd = $state(0);
  cacheRatio = $state(0);

  models = $state<ProviderModel[]>([]);
  modelsState = $state<'idle' | 'loading' | 'ready' | 'error'>('idle');
  modelsError = $state('');
  thinkingLevel = $state<ThinkingLevel>('off');

  sessionTurns = $state(0);
  sessionBeganAt = $state<number | null>(null);
  sessionTotals = $state<TokenUsage>({ input: 0, cached_input: 0, output: 0, thinking: 0 });
  ctxUsed = $state(0);

  draft = $state('');
  pendingPermission = $state<PermissionRequest | null>(null);
  awaitingPrompt = $state<string | null>(null);
  queue = $state<QueuedMessage[]>([]);

  planMode = $state(false);
  planReady = $state(false);

  leftCollapsed = $state(false);
  rightCollapsed = $state(false);
  leftWidth = $state(260);
  rightWidth = $state(300);

  sessionId = $state<string | null>(null);
  sessionMeta = $state<SessionMeta | null>(null);
  sessions = $state<SessionMeta[]>([]);
  sessionsState = $state<'idle' | 'loading' | 'ready' | 'error'>('idle');
  pickerMode = $state<SessionMode | null>(null);
  sessionBusy = $state(false);
  providers = $state<ProviderInfo[]>([]);

  updateAvailable = $state<{ version: string; notes: string | null } | null>(null);
  updateState = $state<'downloading' | 'installing' | 'error' | null>(null);
  updateDownloaded = $state(0);
  updateTotal = $state<number | null>(null);

  mode = $state<AppMode>('home');
  theme = $state<string>('auto');
  workspacePath = $state<string | null>(null);

  private unlisten: UnlistenFn | null = null;
  private unlistenUpdate: UnlistenFn | null = null;
  private fixtureTimers: ReturnType<typeof setTimeout>[] = [];
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private roundStart = 0;
  private pendingToolUsage: TokenUsage | null = null;
  private turnBlockId: number | null = null;
  private turnIsPlan = false;
  private planExecuting = false;

  get running(): boolean {
    return this.sessionState === 'running';
  }

  get selectedModel(): ProviderModel | undefined {
    return (
      this.models.find((m) => m.id === this.model && m.provider === this.providerName) ??
      this.models.find((m) => m.id === this.model)
    );
  }

  get ctxLimit(): number {
    return this.selectedModel?.context_tokens ?? this.budgetTokens;
  }

  get ctxRatio(): number {
    return this.ctxLimit > 0 ? Math.min(1, this.ctxUsed / this.ctxLimit) : 0;
  }

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
      this.unlistenUpdate = await listen<{
        kind: string;
        downloaded?: number;
        total?: number | null;
      }>('update-event', (e) => {
        if (e.payload.kind === 'downloading') {
          this.updateState = 'downloading';
          this.updateDownloaded = e.payload.downloaded ?? 0;
          this.updateTotal = e.payload.total ?? null;
        }
      });
      void this.checkForUpdate();
      void import('./ipc')
        .then((m) => m.presenceSetMode(this.mode))
        .catch(() => {});
    } else {
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
      (window as unknown as { __hateSession?: SessionStore }).__hateSession = this;
    }
  }

  destroy(): void {
    this.unlisten?.();
    this.unlistenUpdate?.();
    this.fixtureTimers.forEach(clearTimeout);
    this.fixtureTimers = [];
  }

  async checkForUpdate(): Promise<void> {
    try {
      const m = await import('./ipc');
      const info = await m.updateCheck();
      if (info) this.updateAvailable = { version: info.version, notes: info.notes };
    } catch {
    }
  }

  async installUpdate(): Promise<void> {
    this.updateState = 'installing';
    try {
      const m = await import('./ipc');
      await m.updateInstall();
    } catch (e) {
      this.updateState = 'error';
      this.note(`update failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  send(override?: string, pinsOverride?: PinRef[]): void {
    const text = (override ?? this.draft).trim();
    if (!text) return;
    const pins = pinsOverride ?? this.collectPins(text);
    if (override === undefined) {
      this.draft = '';
      this.pinned = [];
    }
    if (this.sessionState === 'running' || this.sessionState === 'awaiting_permission') {
      this.queue.push({ text, pins });
      return;
    }
    const plan = this.planMode;
    const wire = plan ? `/plan ${text}` : text;
    this.planReady = false;
    const begin = () => {
      const id = nextBlockId++;
      this.blocks.push(
        plan
          ? { id, kind: 'user', text, plan: true, ...(pins.length ? { pins } : {}) }
          : { id, kind: 'user', text, ...(pins.length ? { pins } : {}) },
      );
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
        return;
      }
      void import('./ipc')
        .then((m) => m.sessionSend(wire, pins))
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

  private collectPins(text: string): PinRef[] {
    const pins = [...this.pinned];
    if (this.tree.length === 0) return pins;
    const known = this.flatPaths();
    for (const token of text.match(/@([^\s@]+)/g) ?? []) {
      const path = token.slice(1).replace(/[.,;:]+$/, '');
      if (!known.has(path)) continue;
      if (!pins.some((p) => p.path === path)) pins.push({ path, start: null, end: null });
    }
    return pins;
  }

  private flatPaths(): Set<string> {
    const out = new Set<string>();
    const walk = (nodes: FileNode[]): void => {
      for (const n of nodes) {
        if (n.kind === 'file') out.add(n.path);
        if (n.children) walk(n.children);
      }
    };
    walk(this.tree);
    return out;
  }

  isPinned(path: string): boolean {
    return this.pinned.some((p) => p.path === path);
  }

  pinFile(path: string, start?: number | null, end?: number | null): void {
    const next: PinRef = { path, start: start ?? null, end: end ?? null };
    const i = this.pinned.findIndex((p) => p.path === path);
    if (i >= 0) this.pinned.splice(i, 1, next);
    else this.pinned.push(next);
  }

  togglePin(path: string): void {
    if (this.isPinned(path)) this.unpin(path);
    else this.pinFile(path);
  }

  unpin(path: string): void {
    this.pinned = this.pinned.filter((p) => p.path !== path);
  }

  async openFile(path: string, line: number | null = null): Promise<void> {
    this.selectedPath = path;
    this.viewer = { kind: 'file', path, line, view: null, diff: null, loading: true, error: null };
    if (!inTauri()) {
      this.viewer.loading = false;
      this.viewer.error = 'file preview needs the desktop app';
      return;
    }
    const m = await import('./ipc');
    try {
      const offset = line && line > 300 ? Math.max(1, line - 50) : 1;
      const view = await m.workspaceRead(path, offset, 400);
      const v = this.viewer;
      if (!v || v.kind !== 'file' || v.path !== path) return;
      this.viewer = { ...v, view, loading: false };
    } catch (e) {
      const v = this.viewer;
      if (!v || v.kind !== 'file' || v.path !== path) return;
      this.viewer = { ...v, loading: false, error: e instanceof Error ? e.message : String(e) };
    }
  }

  async openDiff(path: string): Promise<void> {
    this.selectedPath = path;
    this.viewer = { kind: 'diff', path, line: null, view: null, diff: null, loading: true, error: null };
    if (!inTauri()) {
      this.viewer.loading = false;
      this.viewer.error = 'diffs need the desktop app';
      return;
    }
    const m = await import('./ipc');
    try {
      const diff = await m.workspaceDiff(path);
      const v = this.viewer;
      if (!v || v.kind !== 'diff' || v.path !== path) return;
      this.viewer = { ...v, diff, loading: false };
    } catch (e) {
      const v = this.viewer;
      if (!v || v.kind !== 'diff' || v.path !== path) return;
      this.viewer = { ...v, loading: false, error: e instanceof Error ? e.message : String(e) };
    }
  }

  closeViewer(): void {
    this.viewer = null;
  }

  async loadMoreLines(): Promise<void> {
    const v = this.viewer;
    if (!v || v.kind !== 'file' || !v.view || !inTauri()) return;
    const shown = v.view.start + v.view.lines.length;
    const m = await import('./ipc');
    try {
      const next = await m.workspaceRead(v.path, shown, 400);
      const cur = this.viewer;
      if (!cur || cur.kind !== 'file' || cur.path !== v.path || !cur.view) return;
      this.viewer = {
        ...cur,
        view: {
          ...cur.view,
          lines: [...cur.view.lines, ...next.lines],
          truncated: next.truncated,
          total_lines: next.total_lines,
        },
      };
    } catch {
    }
  }

  async refreshViewer(): Promise<void> {
    const v = this.viewer;
    if (!v) return;
    if (v.kind === 'file') await this.openFile(v.path, v.line);
    else await this.openDiff(v.path);
  }

  async refreshChanges(): Promise<void> {
    if (!inTauri() || !this.workspacePath) return;
    const m = await import('./ipc');
    try {
      this.changedFiles = await m.workspaceChanges();
    } catch {
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

  togglePlan(): void {
    this.planMode = !this.planMode;
    if (!this.planMode) this.planReady = false;
    this.scheduleUiSave();
  }

  runPlan(): void {
    if (!this.planReady || this.running) return;
    this.planMode = false;
    this.planReady = false;
    this.planExecuting = true;
    this.plan = this.plan.map((s, i) => ({ ...s, status: i === 0 ? 'active' : 'pending' }));
    this.send(RUN_PLAN_TEXT);
  }

  revisePlan(): void {
    this.planReady = false;
  }

  discardPlan(): void {
    this.planReady = false;
    this.planMode = false;
    this.plan = [];
    this.scheduleUiSave();
  }

  removeQueued(index: number): void {
    this.queue.splice(index, 1);
  }

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

  retryLast(): void {
    const text = this.lastUserText();
    if (!text) {
      this.note('nothing to retry yet');
      return;
    }
    let last: TranscriptBlock | undefined;
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      if (this.blocks[i]?.kind === 'user') {
        last = this.blocks[i];
        break;
      }
    }
    this.send(text, last?.pins ?? []);
  }

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

  async exportTranscript(): Promise<void> {
    if (this.blocks.length === 0) {
      this.note('nothing to export yet');
      return;
    }
    const ok = await copyText(transcriptMarkdown(this.blocks));
    this.note(ok ? 'transcript copied to the clipboard as Markdown' : 'clipboard unavailable');
  }

  note(text: string): void {
    this.blocks.push({ id: nextBlockId++, kind: 'text', text, streaming: false, local: true });
  }

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

  async refreshModels(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.providers = await m.providersList().catch(() => []);
    this.modelsState = 'loading';
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

  async cycleModel(): Promise<void> {
    const list = this.models;
    if (list.length === 0) return;
    const i = list.findIndex(
      (m) => m.id === this.model && (!m.provider || m.provider === this.providerName),
    );
    const next = list[(i + 1) % list.length];
    if (next) await this.setModel(next.id, next.provider);
  }

  async compact(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.sessionCompact().catch(() => {});
  }

  sessionMode(): SessionMode {
    return this.mode === 'code' ? 'code' : 'chat';
  }

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

  async openSession(id: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.sessionBusy = true;
    try {
      const res = await m.sessionOpen(id);
      this.sessionId = res.meta.id;
      this.sessionMeta = res.meta;
      this.applyUiState(res.ui_state);
      if (res.meta.provider) {
        this.providerName = res.meta.provider;
        this.provider = res.meta.provider;
      }
      if (res.meta.model) this.model = res.meta.model;
      if (
        res.meta.mode === 'code' &&
        res.meta.workspace &&
        res.meta.workspace !== this.workspacePath
      ) {
        this.workspacePath = res.meta.workspace;
        void m.workspaceSet(res.meta.workspace).catch(() => {});
        void this.refreshIndex();
      }
      await this.syncSelection(m);
      localStorage.setItem(`hate:session:${res.meta.mode}`, id);
      this.pickerMode = null;
      void this.refreshSessions(res.meta.mode as SessionMode);
    } finally {
      this.sessionBusy = false;
    }
  }

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
      await this.syncSelection(m);
      localStorage.setItem(`hate:session:${meta.mode}`, meta.id);
      this.pickerMode = null;
      void this.refreshSessions(meta.mode);
    } finally {
      this.sessionBusy = false;
    }
  }

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

  private resetConversation(): void {
    this.blocks = [];
    nextBlockId = 1;
    this.plan = [];
    this.usage = { input: 0, cached_input: 0, output: 0, thinking: 0 };
    this.checkpoints = [];
    this.changedFiles = [];
    this.viewer = null;
    this.pinned = [];
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

  private applyUiState(raw: string | null): void {
    this.viewer = null;
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
    if (inTauri()) {
      void import('./ipc')
        .then((m) => m.presenceSetMode(mode))
        .catch(() => {});
    }
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

  async pickWorkspace(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    const p = await m.workspacePick().catch(() => null);
    if (p) {
      this.workspacePath = p;
      this.closeViewer();
      this.pinned = [];
      await m.workspaceSet(p).catch(() => {});
      void this.refreshIndex();
    }
  }

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
      if (this.workspacePath !== root) return;
      this.tree = idx.tree;
      this.symbols = idx.symbols;
      this.indexTruncated = idx.truncated;
      void this.refreshChanges();
    } catch {
    }
  }

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
        this.ctxUsed = e.usage.input;
        this.recomputeDerived();
        this.attributeUsage(e.usage, e.thinking_est ?? 0);
        break;
      case 'compaction_started':
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
        this.pushError(failureText(e.reason));
        this.sessionState = 'failed';
        this.finishStreamingBlocks();
        if (this.planExecuting) {
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
    this.scheduleUiSave();
  }

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
    if (textBlock) textBlock.streaming = false;
    if (thinkBlock) thinkBlock.streaming = false;
    this.roundStart = this.blocks.length;
  }

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
    if (completed) {
      void this.refreshChanges();
      if (this.viewer) void this.refreshViewer();
    }
    if (completed && !this.planReady && this.queue.length > 0) {
      const next = this.queue.shift();
      if (next) queueMicrotask(() => this.send(next.text, next.pins));
    }
  }

  private recomputeDerived(): void {
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

  private startFixturePlayback(): void {
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
    this.fixtureTimers.push(
      setTimeout(() => {
        this.sessionState = 'idle';
        this.finishStreamingBlocks();
      }, t + 800),
    );
  }
}

export const session = new SessionStore();
