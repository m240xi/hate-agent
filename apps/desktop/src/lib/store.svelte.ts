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
  BranchGroup,
  ChangedFile,
  CheckpointEntry,
  ContextRef,
  CustomCommandInfo,
  FailureReason,
  FileDiff,
  FileNode,
  FileView,
  PermissionRequest,
  PinRef,
  PlanStep,
  SessionEvent,
  ShellInfo,
  SymbolHit,
  TerminalTabInfo,
  TokenUsage,
  ToolAuditEntry,
  TranscriptBlock,
  TurnOutcome,
  WorkspaceRuleInfo,
  WorktreeStatus,
} from './types';
import type {
  ImageAttachment,
  Prefs,
  ProviderInfo,
  ProviderModel,
  SessionMeta,
  ThinkingLevel,
  UsageOverview,
} from './ipc';
import { isThemeId } from './themes';
import { copyText, parsePlan, transcriptHtml, transcriptJson, transcriptMarkdown } from './format';

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

export type AppMode = 'home' | 'chat' | 'settings';

const MODES: readonly AppMode[] = ['home', 'chat', 'settings'];
const LAST_SESSION = 'hate:session:last';

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
  contexts: ContextRef[];
  images: ImageAttachment[];
}

export interface PendingImage extends ImageAttachment {
  /** data URI used for the inline preview */
  url: string;
}

class SessionStore {
  blocks = $state<TranscriptBlock[]>([]);

  tree = $state<FileNode[]>([]);
  symbols = $state<SymbolHit[]>([]);
  indexTruncated = $state(false);
  selectedPath = $state<string | null>(null);
  viewer = $state<ViewerState | null>(null);
  pinned = $state<PinRef[]>([]);
  contexts = $state<ContextRef[]>([]);

  // Multi-tab workspace terminal
  terminals = $state<TerminalTabInfo[]>([]);
  activeTerminalId = $state<string | null>(null);
  terminalOpen = $state(false);
  terminalDock = $state<'bottom' | 'right'>('bottom');
  terminalHeight = $state(240);
  terminalShells = $state<ShellInfo[]>([]);

  // Quick open, search, branching, worktrees, rules, custom commands
  quickOpen = $state(false);
  transcriptSearchOpen = $state(false);
  branchGroups = $state<BranchGroup[]>([]);
  worktree = $state<WorktreeStatus | null>(null);
  workspaceRules = $state<WorkspaceRuleInfo[]>([]);
  customCommands = $state<CustomCommandInfo[]>([]);

  plan = $state<PlanStep[]>([]);
  usage = $state<TokenUsage>({ input: 0, cached_input: 0, output: 0, thinking: 0 });
  budgetTokens = 200_000;
  checkpoints = $state<CheckpointEntry[]>([]);
  checkpointDiffPreview = $state<{ id: string; diff: string } | null>(null);
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
  agentCtxSize = $state(0);
  tokensSaved = $state(0);
  cacheBreakReason = $state<string | null>(null);
  toolAudits = $state<ToolAuditEntry[]>([]);

  draft = $state('');
  attachments = $state<PendingImage[]>([]);
  pendingPermission = $state<PermissionRequest | null>(null);
  awaitingPrompt = $state<string | null>(null);
  queue = $state<QueuedMessage[]>([]);

  planMode = $state(false);
  planReady = $state(false);

  rightCollapsed = $state(false);
  sidebarCollapsed = $state(false);
  panelTab = $state<'files' | 'activity' | 'terminal'>('files');
  chatPanelOpen = $state(false);

  get panelOpen(): boolean {
    return this.workspacePath ? !this.rightCollapsed : this.chatPanelOpen;
  }

  setPanel(open: boolean): void {
    if (this.workspacePath) this.rightCollapsed = !open;
    else this.chatPanelOpen = open;
  }

  openActivity(): void {
    this.panelTab = 'activity';
    this.setPanel(true);
  }
  rightWidth = $state(300);

  sessionId = $state<string | null>(null);
  sessionMeta = $state<SessionMeta | null>(null);
  sessions = $state<SessionMeta[]>([]);
  sessionsState = $state<'idle' | 'loading' | 'ready' | 'error'>('idle');
  sessionBusy = $state(false);
  providers = $state<ProviderInfo[]>([]);

  updateAvailable = $state<{ version: string; notes: string | null } | null>(null);
  updateState = $state<'downloading' | 'installing' | 'error' | null>(null);
  repoTrust = $state<import('./ipc').RepoTrustStatus | null>(null);
  trustDismissed = $state(false);
  updateDownloaded = $state(0);
  updateTotal = $state<number | null>(null);
  whatsNew = $state<{ version: string; notes: string | null } | null>(null);

  // Live turn telemetry, used by the "still working" bar.
  turnStartedAt = $state<number | null>(null);
  firstTokenMs = $state<number | null>(null);
  toolsThisTurn = $state(0);
  notice = $state<{ id: number; text: string } | null>(null);

  mode = $state<AppMode>('home');
  theme = $state<string>('auto');
  workspacePath = $state<string | null>(null);

  // Spending guardrails and completion attention.
  prefs = $state<Prefs>({
    budget_session_usd: null,
    budget_day_usd: null,
    sound_on_done: false,
    notify_on_done: true,
    theme: null,
    glass: null,
    density: null,
    motion: null,
  });
  todayCostUsd = $state(0);
  budgetPrompt = $state(false);
  private budgetOverride = false;

  usageOverview = $state<UsageOverview | null>(null);
  usageState = $state<'idle' | 'loading' | 'ready' | 'error'>('idle');
  usageError = $state('');
  usageDays = $state(30);

  density = $state<'comfortable' | 'compact'>('comfortable');

  /**
   * Motion policy. Two tiers, because "reduce motion" means different things
   * for a decorative scene than for a fade:
   *  - `motion`      the choice itself, mirrored to `data-motion`.
   *  - `ambientMotion` whether *decorative* motion (fades, drift, twinkle,
   *    shimmer) may run, mirrored to `data-ambient-motion`. Off under an
   *    explicit "Reduced" and, on "System", when the OS asks for less motion.
   * Character animation (the mascot, the landing scene) is not decorative
   * movement — it is the app's identifying art — so it follows `motion` only.
   */
  motion = $state<'system' | 'full' | 'reduced'>('system');
  ambientMotion = $state<'on' | 'off'>('on');
  paletteOpen = $state(false);

  private unlisten: UnlistenFn | null = null;
  private motionUnlisten: (() => void) | null = null;
  private unlistenUpdate: UnlistenFn | null = null;
  private unlistenTermOut: UnlistenFn | null = null;
  private unlistenTermExit: UnlistenFn | null = null;
  private termOutputListeners = new Set<(id: string, data: string) => void>();
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
    return this.agentCtxSize || (this.selectedModel?.context_tokens ?? this.budgetTokens);
  }

  get ctxRatio(): number {
    return this.ctxLimit > 0 ? Math.min(1, this.ctxUsed / this.ctxLimit) : 0;
  }

  async start(): Promise<void> {
    const saved = localStorage.getItem('hate:mode');
    if (saved === 'code') this.mode = 'chat';
    else if (MODES.includes(saved as AppMode)) this.mode = saved as AppMode;
    const savedTheme = localStorage.getItem('hate:theme');
    if (isThemeId(savedTheme)) this.theme = savedTheme;
    // Drop a theme id we no longer ship (e.g. a retired palette) so the
    // fallback to Auto isn't re-evaluated on every launch.
    else if (savedTheme) localStorage.removeItem('hate:theme');
    this.glass = localStorage.getItem('hate:glass') === 'on';
    const savedThinking = localStorage.getItem('hate:thinking');
    if (isThinkingLevel(savedThinking)) this.thinkingLevel = savedThinking;
    const savedMotion = localStorage.getItem('hate:motion');
    if (savedMotion === 'full' || savedMotion === 'reduced' || savedMotion === 'system') {
      this.motion = savedMotion;
    }
    const savedDock = localStorage.getItem('hate:term:dock');
    if (savedDock === 'bottom' || savedDock === 'right') this.terminalDock = savedDock;
    const savedTermH = Number(localStorage.getItem('hate:term:height'));
    if (savedTermH >= 140 && savedTermH <= 560) this.terminalHeight = savedTermH;
    this.applyTheme();
    this.applyMotion();
    this.watchSystemMotion();
    if (inTauri()) {
      const m = await import('./ipc');
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
      void this.refreshSessions();
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
      this.unlistenTermOut = await listen<{ id: string; data: string }>('terminal-output', (e) => {
        for (const fn of this.termOutputListeners) fn(e.payload.id, e.payload.data);
      });
      this.unlistenTermExit = await listen<{ id: string; exit_code: number | null }>('terminal-exit', (e) => {
        const t = this.terminals.find((tab) => tab.id === e.payload.id);
        if (t) {
          t.alive = false;
          t.exit_code = e.payload.exit_code;
        }
      });
      void m.terminalListShells().then((list) => (this.terminalShells = list)).catch(() => {});
      void m.terminalList().then((list) => {
        this.terminals = list;
        if (!this.activeTerminalId && list[0]) this.activeTerminalId = list[0].id;
      }).catch(() => {});
      void this.refreshWorkspaceExtras();
      void this.checkForUpdate();
      void this.checkWhatsNew();
      void this.loadPrefs();
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
      void this.loadPrefs();
      (window as unknown as { __hateSession?: SessionStore }).__hateSession = this;
    }
  }

  destroy(): void {
    this.unlisten?.();
    this.motionUnlisten?.();
    this.motionUnlisten = null;
    this.unlistenUpdate?.();
    this.unlistenTermOut?.();
    this.unlistenTermExit?.();
    this.termOutputListeners.clear();
    this.fixtureTimers.forEach(clearTimeout);
    this.fixtureTimers = [];
  }

  async checkWhatsNew(): Promise<void> {
    const m = await import('./ipc');
    const info = await m.appInfo().catch(() => null);
    if (!info) return;
    const prev = localStorage.getItem('hate:lastVersion');
    localStorage.setItem('hate:lastVersion', info.version);
    if (!prev || prev === info.version) return;
    let notes: string | null = null;
    try {
      const raw = localStorage.getItem('hate:pendingUpdate');
      if (raw) {
        const p = JSON.parse(raw) as { version?: string; notes?: string | null };
        if (p.version === info.version && p.notes) notes = p.notes;
      }
    } catch {
      // malformed leftover; show the modal without notes
    }
    localStorage.removeItem('hate:pendingUpdate');
    this.whatsNew = { version: info.version, notes };
  }

  dismissWhatsNew(): void {
    this.whatsNew = null;
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
    if (this.updateAvailable) {
      localStorage.setItem(
        'hate:pendingUpdate',
        JSON.stringify({
          version: this.updateAvailable.version,
          notes: this.updateAvailable.notes,
        }),
      );
    }
    this.updateState = 'installing';
    try {
      const m = await import('./ipc');
      await m.updateInstall();
    } catch (e) {
      this.updateState = 'error';
      this.note(`update failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  steer(override?: string): void {
    const raw = (override ?? this.draft).trim();
    if (!raw) return;
    if (override === undefined) {
      this.draft = '';
    }
    if (!this.running) {
      this.send(raw);
      return;
    }
    this.blocks.push({
      id: nextBlockId++,
      kind: 'text',
      text: `[steer] ${raw}`,
      streaming: false,
      local: true,
    });
    if (!inTauri()) return;
    void import('./ipc')
      .then((m) => m.sessionSteer(raw))
      .catch((err: unknown) => {
        this.note(`steer failed: ${err instanceof Error ? err.message : String(err)}`);
      });
  }

  send(override?: string, pinsOverride?: PinRef[], contextsOverride?: ContextRef[]): void {
    if (this.budgetBlocked) {
      this.budgetPrompt = true;
      return;
    }
    const rawText = (override ?? this.draft).trim();
    const images = this.attachments.map(({ mime, data }) => ({ mime, data }));
    const pins = pinsOverride ?? this.collectPins(rawText);
    const contexts = contextsOverride ?? this.collectContexts(rawText);
    if (!rawText && images.length === 0 && contexts.length === 0 && pins.length === 0) return;
    const text = rawText || (images.length > 0 ? '' : 'Analyze the attached context.');
    const dropped = override === undefined ? this.attachments : [];
    if (override === undefined) {
      this.draft = '';
      this.pinned = [];
      this.contexts = [];
      this.attachments = [];
    }
    if (this.sessionState === 'running' || this.sessionState === 'awaiting_permission') {
      this.queue.push({ text, pins, contexts, images });
      return;
    }
    const plan = this.planMode;
    const wire = plan ? `/plan ${text}` : text;
    this.planReady = false;
    const begin = () => {
      const id = nextBlockId++;
      this.blocks.push({
        id,
        kind: 'user',
        text,
        ...(plan ? { plan: true } : {}),
        ...(pins.length ? { pins } : {}),
        ...(contexts.length ? { contexts } : {}),
        ...(dropped.length ? { images: dropped.map((a) => a.url) } : {}),
      });
      this.turnBlockId = id;
      this.turnIsPlan = plan;
      this.roundStart = this.blocks.length;
      this.pendingToolUsage = null;
      this.sessionState = 'running';
      this.turnStartedAt = Date.now();
      this.firstTokenMs = null;
      this.toolsThisTurn = 0;
      this.sessionTurns += 1;
      this.sessionBeganAt ??= Date.now();
      this.finishStreamingBlocks();
      this.scheduleUiSave();

      if (!inTauri()) {
        return;
      }
      void import('./ipc')
        .then((m) => m.sessionSend(wire, pins, contexts, images))
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

  private contextKey(c: ContextRef): string {
    switch (c.kind) {
      case 'git_diff':
      case 'git_staged':
        return c.kind;
      case 'terminal':
        return `terminal:${c.id ?? ''}`;
      case 'url':
        return `url:${c.url}`;
      case 'folder':
        return `folder:${c.path}`;
    }
  }

  addContext(ctx: ContextRef): void {
    const key = this.contextKey(ctx);
    if (this.contexts.some((c) => this.contextKey(c) === key)) return;
    this.contexts = [...this.contexts, ctx];
  }

  removeContext(index: number): void {
    this.contexts = this.contexts.filter((_, i) => i !== index);
  }

  private collectContexts(text: string): ContextRef[] {
    const out = [...this.contexts];
    const pushUnique = (c: ContextRef): void => {
      const k = this.contextKey(c);
      if (!out.some((x) => this.contextKey(x) === k)) out.push(c);
    };
    for (const token of text.match(/@([^\s@]+)/g) ?? []) {
      const clean = token.slice(1).replace(/[.,;:]+$/, '');
      if (clean === 'git:diff') pushUnique({ kind: 'git_diff' });
      else if (clean === 'git:staged') pushUnique({ kind: 'git_staged' });
      else if (clean === 'terminal') pushUnique({ kind: 'terminal', id: this.activeTerminalId, lines: 120 });
      else if (clean.startsWith('folder:')) {
        const folder = clean.slice('folder:'.length);
        if (folder) pushUnique({ kind: 'folder', path: folder });
      } else if (clean.startsWith('url:')) {
        const url = clean.slice('url:'.length);
        if (/^https?:\/\//i.test(url)) pushUnique({ kind: 'url', url });
      }
    }
    return out;
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

  get reviewIndex(): number {
    const p = this.viewer?.path;
    if (!p) return -1;
    return this.changedFiles.findIndex((f) => f.path === p);
  }

  nextChangedFile(): void {
    if (this.changedFiles.length === 0) return;
    const cur = this.reviewIndex;
    const next = this.changedFiles[(cur + 1) % this.changedFiles.length];
    if (next) void this.openDiff(next.path);
  }

  prevChangedFile(): void {
    if (this.changedFiles.length === 0) return;
    const cur = this.reviewIndex;
    const prev = this.changedFiles[(cur <= 0 ? this.changedFiles.length : cur) - 1];
    if (prev) void this.openDiff(prev.path);
  }

  async saveViewerFile(content: string): Promise<boolean> {
    const v = this.viewer;
    if (!v || !inTauri()) return false;
    const m = await import('./ipc');
    try {
      await m.workspaceWriteFile(v.path, content);
      await this.openFile(v.path, v.line);
      void this.refreshChanges();
      void this.refreshIndex();
      this.setNotice(`Saved ${v.path}`);
      return true;
    } catch (e) {
      this.note(`couldn't save ${v.path}: ${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
  }

  async revertChangedFile(path: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    try {
      await m.workspaceRevertFile(path);
      await this.refreshChanges();
      void this.refreshIndex();
      if (this.viewer?.path === path) {
        if (this.changedFiles.some((f) => f.path === path)) await this.openDiff(path);
        else this.closeViewer();
      }
      this.setNotice(`Reverted ${path}`);
    } catch (e) {
      this.note(`revert failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async revertDiffHunk(path: string, patch: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    try {
      await m.workspaceRevertHunk(path, patch);
      await this.refreshChanges();
      void this.refreshIndex();
      if (this.viewer?.path === path) {
        if (this.changedFiles.some((f) => f.path === path)) await this.openDiff(path);
        else this.closeViewer();
      }
      this.setNotice('Reverted hunk');
    } catch (e) {
      this.note(`hunk revert failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  askAiAboutLines(path: string, start: number | null, end: number | null): void {
    this.pinFile(path, start, end);
    const loc = start ? (end && end !== start ? `:${start}-${end}` : `:${start}`) : '';
    if (!this.draft.trim()) {
      this.draft = `Explain or review ${path}${loc}: `;
    }
    const ta = document.querySelector<HTMLTextAreaElement>('.composer textarea');
    if (ta) {
      ta.focus();
      ta.setSelectionRange(this.draft.length, this.draft.length);
    }
  }

  // ---- multi-tab terminal --------------------------------------------------

  onTerminalOutput(fn: (id: string, data: string) => void): () => void {
    this.termOutputListeners.add(fn);
    return () => this.termOutputListeners.delete(fn);
  }

  async openTerminal(dock?: 'bottom' | 'right'): Promise<void> {
    if (dock) {
      this.terminalDock = dock;
      localStorage.setItem('hate:term:dock', dock);
    }
    if (this.terminalDock === 'right') {
      this.panelTab = 'terminal';
      this.setPanel(true);
    } else {
      this.terminalOpen = true;
    }
    if (inTauri() && this.terminals.length === 0) {
      await this.createTerminal();
    }
  }

  toggleTerminal(): void {
    const visible =
      this.terminalDock === 'right'
        ? this.panelOpen && this.panelTab === 'terminal'
        : this.terminalOpen;
    if (visible) {
      if (this.terminalDock === 'right') {
        this.setPanel(false);
      } else {
        this.terminalOpen = false;
      }
    } else {
      void this.openTerminal();
    }
  }

  setTerminalDock(dock: 'bottom' | 'right'): void {
    this.terminalDock = dock;
    localStorage.setItem('hate:term:dock', dock);
    if (dock === 'right') {
      this.terminalOpen = false;
      this.panelTab = 'terminal';
      this.setPanel(true);
    } else {
      if (this.panelTab === 'terminal') {
        this.panelTab = this.workspacePath ? 'files' : 'activity';
      }
      this.terminalOpen = true;
    }
  }

  setTerminalHeight(px: number): void {
    const clamped = Math.min(560, Math.max(140, Math.round(px)));
    this.terminalHeight = clamped;
    localStorage.setItem('hate:term:height', String(clamped));
  }

  async createTerminal(shell?: string | null): Promise<void> {
    if (!inTauri()) {
      const id = `demo-${this.terminals.length + 1}`;
      this.terminals = [
        ...this.terminals,
        { id, title: `Terminal ${this.terminals.length + 1}`, shell: shell ?? 'demo', cwd: this.workspacePath ?? '~', alive: true, exit_code: null },
      ];
      this.activeTerminalId = id;
      return;
    }
    const m = await import('./ipc');
    try {
      const tab = await m.terminalCreate(this.workspacePath, shell ?? null);
      this.terminals = [...this.terminals, tab];
      this.activeTerminalId = tab.id;
    } catch (e) {
      this.note(`couldn't open terminal: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async closeTerminal(id: string): Promise<void> {
    if (inTauri()) {
      const m = await import('./ipc');
      await m.terminalClose(id).catch(() => {});
    }
    this.terminals = this.terminals.filter((t) => t.id !== id);
    if (this.activeTerminalId === id) {
      this.activeTerminalId = this.terminals[0]?.id ?? null;
    }
    if (this.terminals.length === 0) {
      if (this.terminalDock === 'bottom') this.terminalOpen = false;
    }
  }

  async writeTerminal(id: string, data: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.terminalWrite(id, data).catch(() => {});
  }

  async resizeTerminal(id: string, cols: number, rows: number): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.terminalResize(id, cols, rows).catch(() => {});
  }

  async readTerminalBuffer(id: string, raw = false, tailLines = 160): Promise<string> {
    if (!inTauri()) return '';
    const m = await import('./ipc');
    return m.terminalReadBuffer(id, raw, tailLines).catch(() => '');
  }

  async cdTerminalToWorkspace(id?: string): Promise<void> {
    const target = id ?? this.activeTerminalId;
    if (!target || !this.workspacePath) return;
    const tab = this.terminals.find((t) => t.id === target);
    const isCmd = tab?.shell.toLowerCase().includes('cmd');
    const cmd = isCmd ? `cd /d "${this.workspacePath}"\r` : `cd "${this.workspacePath}"\r`;
    await this.writeTerminal(target, cmd);
  }

  attachTerminalToChat(id?: string): void {
    const target = id ?? this.activeTerminalId;
    if (!target) {
      this.note('open a terminal tab first');
      return;
    }
    this.addContext({ kind: 'terminal', id: target, lines: 120 });
    document.querySelector<HTMLTextAreaElement>('.composer textarea')?.focus();
  }

  async askAiToFixTerminal(id?: string): Promise<void> {
    const target = id ?? this.activeTerminalId;
    if (!target) {
      this.note('open a terminal tab first');
      return;
    }
    this.send('Inspect the attached terminal output, explain the error, and help fix it.', [], [
      { kind: 'terminal', id: target, lines: 140 },
    ]);
  }

  // ---- bookmarks -----------------------------------------------------------

  toggleBookmark(blockId: number): void {
    const b = this.blocks.find((x) => x.id === blockId);
    if (!b) return;
    b.bookmarked = !b.bookmarked;
    this.scheduleUiSave();
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

  lastReplyId = $derived.by(() => {
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      const b = this.blocks[i];
      if (b?.kind === 'user') return null;
      if (b?.kind === 'text' && !b.local) return b.id;
    }
    return null;
  });

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

  get lastUserBlock(): TranscriptBlock | undefined {
    for (let i = this.blocks.length - 1; i >= 0; i--) {
      if (this.blocks[i]?.kind === 'user') return this.blocks[i];
    }
    return undefined;
  }

  get canRegenerate(): boolean {
    const u = this.lastUserBlock;
    return !!u?.text && !u.images?.length && !this.running;
  }

  async regenerate(): Promise<void> {
    const u = this.lastUserBlock;
    if (!u?.text || !this.canRegenerate) return;
    await this.editUserMessage(u.id, u.text, false);
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

  // ---- spending guardrails -------------------------------------------------

  get sessionBudget(): number | null {
    const b = this.prefs.budget_session_usd;
    return b && b > 0 ? b : null;
  }

  get sessionCostRatio(): number {
    const b = this.sessionBudget;
    return b ? this.costUsd / b : 0;
  }

  get dayBudget(): number | null {
    const b = this.prefs.budget_day_usd;
    return b && b > 0 ? b : null;
  }

  get dayCostRatio(): number {
    const b = this.dayBudget;
    return b ? this.todayCostUsd / b : 0;
  }

  get budgetBlocked(): boolean {
    if (this.budgetOverride) return false;
    return this.sessionCostRatio >= 1 || this.dayCostRatio >= 1;
  }

  get budgetWarning(): boolean {
    return (
      !this.budgetBlocked &&
      (this.sessionCostRatio >= 0.8 || this.dayCostRatio >= 0.8)
    );
  }

  continueOverBudget(): void {
    this.budgetOverride = true;
    this.budgetPrompt = false;
    this.send();
  }

  dismissBudgetPrompt(): void {
    this.budgetPrompt = false;
  }

  async loadPrefs(): Promise<void> {
    this.density =
      localStorage.getItem('hate:density') === 'compact' ? 'compact' : 'comfortable';
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.prefs = await m.prefsGet().catch(() => this.prefs);
    this.adoptAppearance();
    if (this.dayBudget) void this.refreshToday();
  }

  /**
   * Appearance lives in the config file (`Prefs`) with localStorage as the
   * cache that browser mode and the first paint use. Saved prefs win; when a
   * field is empty (a config from before appearance moved here) the current
   * value is pushed up so the next launch agrees with this one.
   */
  private adoptAppearance(): void {
    const p = this.prefs;
    const migrate: Partial<Prefs> = {};
    if (typeof p.theme === 'string' && isThemeId(p.theme)) this.theme = p.theme;
    else if (p.theme == null) migrate.theme = this.theme;
    if (typeof p.glass === 'boolean') this.glass = p.glass;
    else if (p.glass == null) migrate.glass = this.glass;
    if (p.density === 'compact' || p.density === 'comfortable') this.density = p.density;
    else if (p.density == null) migrate.density = this.density;
    if (p.motion === 'system' || p.motion === 'full' || p.motion === 'reduced') {
      this.motion = p.motion;
    } else if (p.motion == null) migrate.motion = this.motion;

    this.applyTheme();
    this.applyMotion();
    this.cacheAppearance();
    if (Object.keys(migrate).length > 0) void this.savePrefs(migrate);
  }

  /** Mirror appearance into localStorage for the next first paint. */
  private cacheAppearance(): void {
    localStorage.setItem('hate:theme', this.theme);
    localStorage.setItem('hate:glass', this.glass ? 'on' : 'off');
    localStorage.setItem('hate:density', this.density);
    localStorage.setItem('hate:motion', this.motion);
  }

  async savePrefs(patch: Partial<Prefs>): Promise<void> {
    this.prefs = { ...this.prefs, ...patch };
    if (patch.budget_session_usd !== undefined || patch.budget_day_usd !== undefined) {
      this.budgetOverride = false;
    }
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.prefsSet(this.prefs).catch((e: unknown) => {
      this.note(`couldn't save preferences: ${e instanceof Error ? e.message : String(e)}`);
    });
    if (this.dayBudget) void this.refreshToday();
  }

  setDensity(d: 'comfortable' | 'compact'): void {
    this.density = d;
    localStorage.setItem('hate:density', d);
    void this.savePrefs({ density: d });
  }

  setMotion(m: 'system' | 'full' | 'reduced'): void {
    this.motion = m;
    localStorage.setItem('hate:motion', m);
    this.applyMotion();
    void this.savePrefs({ motion: m });
  }

  private applyMotion(): void {
    if (typeof document === 'undefined') return;
    const osReduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const off = this.motion === 'reduced' || (this.motion === 'system' && osReduced);
    this.ambientMotion = off ? 'off' : 'on';
    document.documentElement.dataset.motion = this.motion;
    document.documentElement.dataset.ambientMotion = this.ambientMotion;
  }

  private watchSystemMotion(): void {
    this.motionUnlisten?.();
    if (typeof window === 'undefined') return;
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const onChange = (): void => {
      if (this.motion === 'system') this.applyMotion();
    };
    mq.addEventListener('change', onChange);
    this.motionUnlisten = () => mq.removeEventListener('change', onChange);
  }

  /** Today's spend, from the ledger, for the daily ceiling. */
  async refreshToday(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    const overview = await m.usageOverview(1).catch(() => null);
    if (!overview) return;
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    const startSec = Math.floor(start.getTime() / 1000);
    this.todayCostUsd = overview.by_day
      .filter((d) => d.day + 86_400 > startSec)
      .reduce((n, d) => n + d.totals.cost_usd, 0);
  }

  // ---- usage history -------------------------------------------------------

  async loadUsage(days = this.usageDays): Promise<void> {
    this.usageDays = days;
    if (!inTauri()) {
      this.usageState = 'error';
      this.usageError = 'usage history needs the desktop app';
      return;
    }
    this.usageState = 'loading';
    const m = await import('./ipc');
    try {
      this.usageOverview = await m.usageOverview(days);
      this.usageState = 'ready';
    } catch (e) {
      this.usageError = e instanceof Error ? e.message : String(e);
      this.usageState = 'error';
    }
  }

  // ---- transcript edit / branch -------------------------------------------

  /**
   * Rewind the conversation to just before a user message and send edited
   * text. `branch` copies the surviving history into a new session instead of
   * rewriting the current one.
   */
  async editUserMessage(blockId: number, text: string, branch: boolean): Promise<void> {
    const idx = this.blocks.findIndex((b) => b.id === blockId);
    if (idx < 0) return;
    const trimmed = text.trim();
    if (!trimmed) return;
    const userIndex = this.blocks.slice(0, idx).filter((b) => b.kind === 'user').length;

    let result: { session_id: string; branched: boolean } | null = null;
    if (inTauri()) {
      const m = await import('./ipc');
      try {
        result = await m.sessionRewindMessage(userIndex, branch, null);
      } catch (e) {
        this.note(`couldn't rewind: ${e instanceof Error ? e.message : String(e)}`);
        return;
      }
    }

    const pins = this.blocks[idx]?.pins ?? [];
    this.blocks = this.blocks.slice(0, idx);
    this.queue = [];
    this.sessionTurns = userIndex;
    if (result?.branched) {
      this.sessionId = result.session_id;
      this.costUsd = 0;
      this.usage = { input: 0, cached_input: 0, output: 0, thinking: 0 };
      this.sessionTotals = { input: 0, cached_input: 0, output: 0, thinking: 0 };
      this.cacheRatio = 0;
      this.sessionBeganAt = null;
      this.note('branched into a new session');
      void this.refreshBranches();
      void this.refreshSessions();
    }
    this.send(trimmed, pins);
  }

  async refreshBranches(): Promise<void> {
    if (!inTauri() || !this.sessionId) {
      this.branchGroups = [];
      return;
    }
    const m = await import('./ipc');
    this.branchGroups = await m.sessionBranches(this.sessionId).catch(() => []);
  }

  async branchAfterBlock(blockId: number): Promise<void> {
    if (this.running) {
      this.note('wait for the current reply to finish before branching');
      return;
    }
    const idx = this.blocks.findIndex((b) => b.id === blockId);
    if (idx < 0) return;
    const userCount = this.blocks.slice(0, idx + 1).filter((b) => b.kind === 'user').length;
    const userIndex = userCount - 1;
    if (userIndex < 0) return;
    let turnEnd = this.blocks.length;
    for (let i = idx + 1; i < this.blocks.length; i++) {
      if (this.blocks[i]?.kind === 'user') {
        turnEnd = i;
        break;
      }
    }
    const kept = this.blocks.slice(0, turnEnd);
    if (!inTauri()) {
      this.blocks = kept;
      this.note('branched from here');
      return;
    }
    const prevBlocks = this.blocks;
    const prevTurns = this.sessionTurns;
    this.blocks = kept;
    this.sessionTurns = userIndex + 1;
    const snap = this.uiSnapshot();
    const userBlock = kept.filter((b) => b.kind === 'user').at(-1);
    const baseTitle = (userBlock?.text ?? 'branch').slice(0, 48).trim() || 'branch';
    const m = await import('./ipc');
    try {
      const res = await m.sessionBranchAfter(userIndex, `${baseTitle} (branch)`, snap);
      this.sessionId = res.session_id;
      this.queue = [];
      this.costUsd = 0;
      this.usage = { input: 0, cached_input: 0, output: 0, thinking: 0 };
      this.sessionTotals = { input: 0, cached_input: 0, output: 0, thinking: 0 };
      this.cacheRatio = 0;
      this.sessionBeganAt = null;
      localStorage.setItem(LAST_SESSION, res.session_id);
      this.note('branched into a new session from here');
      void this.refreshBranches();
      void this.refreshSessions();
    } catch (e) {
      this.blocks = prevBlocks;
      this.sessionTurns = prevTurns;
      this.note(`couldn't branch: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async switchBranch(sessionId: string): Promise<void> {
    if (!sessionId || sessionId === this.sessionId) return;
    await this.openSession(sessionId);
  }

  // ---- export --------------------------------------------------------------

  async exportTo(format: 'md' | 'html' | 'json'): Promise<void> {
    if (this.blocks.length === 0) {
      this.note('nothing to export yet');
      return;
    }
    if (!inTauri()) {
      this.note('exporting needs the desktop app');
      return;
    }
    const m = await import('./ipc');
    const content =
      format === 'md'
        ? transcriptMarkdown(this.blocks)
        : format === 'html'
          ? transcriptHtml(this.blocks, this.exportMeta())
          : transcriptJson(this.blocks, this.exportMeta());
    try {
      const path = await m.exportSave(format, content);
      if (path) this.setNotice(`saved to ${path}`);
    } catch (e) {
      this.note(`export failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  private exportMeta(): {
    title: string;
    model: string;
    provider: string;
    sessionId: string | null;
    costUsd: number;
    turns: number;
    usage: TokenUsage;
  } {
    return {
      title: this.sessionMeta?.title || 'hate-agent session',
      model: this.model,
      provider: this.providerName || this.provider,
      sessionId: this.sessionId,
      costUsd: this.costUsd,
      turns: this.sessionTurns,
      usage: this.sessionTotals,
    };
  }

  setNotice(text: string): void {
    this.notice = { id: Date.now(), text };
  }

  // ---- completion chime ----------------------------------------------------

  private chime(): void {
    if (!this.prefs.sound_on_done || typeof window === 'undefined') return;
    try {
      const Ctx =
        window.AudioContext ??
        (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
      if (!Ctx) return;
      const ctx = new Ctx();
      const now = ctx.currentTime;
      const notes = [660, 990];
      notes.forEach((freq, i) => {
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.type = 'sine';
        osc.frequency.value = freq;
        const at = now + i * 0.11;
        gain.gain.setValueAtTime(0, at);
        gain.gain.linearRampToValueAtTime(0.06, at + 0.02);
        gain.gain.exponentialRampToValueAtTime(0.0001, at + 0.28);
        osc.connect(gain).connect(ctx.destination);
        osc.start(at);
        osc.stop(at + 0.3);
      });
      setTimeout(() => void ctx.close().catch(() => {}), 900);
    } catch {
      // audio is best-effort
    }
  }

  async exportTranscript(): Promise<void> {
    if (this.blocks.length === 0) {
      this.note('nothing to export yet');
      return;
    }
    const ok = await copyText(transcriptMarkdown(this.blocks));
    this.note(ok ? 'transcript copied to the clipboard as Markdown' : 'clipboard unavailable');
  }

  dropping = $state(false);

  async handleDrop(paths: string[]): Promise<void> {
    this.dropping = false;
    if (!inTauri() || paths.length === 0 || this.mode !== 'chat') return;
    const m = await import('./ipc');
    const items = await m.dropInspect(paths).catch(() => []);
    const folders = items.filter((i) => i.kind === 'folder');
    const images = items.filter((i) => i.kind === 'image' && i.mime && i.data);
    const skipped = items.filter((i) => i.kind === 'other');
    const room = Math.max(0, 4 - this.attachments.length);
    for (const img of images.slice(0, room)) {
      this.attachments = [
        ...this.attachments,
        { mime: img.mime!, data: img.data!, url: `data:${img.mime};base64,${img.data}` },
      ];
    }
    if (folders[0]) await this.attachFolder(folders[0].path);
    const notes: string[] = [];
    if (images.length > room) notes.push('attach at most 4 images per message');
    if (folders.length > 1) notes.push('only one folder per chat — used the first one');
    if (skipped.length > 0) notes.push(skipped[0]?.error ?? "that file type can't be attached");
    if (notes.length > 0) this.note(notes.join(' · '));
  }

  addImage(file: File): Promise<void> {
    return new Promise((resolve) => {
      if (!/^image\/(png|jpeg|webp|gif|bmp)$/.test(file.type) || file.size > 8 * 1024 * 1024) {
        this.note('only PNG, JPEG, WebP, GIF or BMP images up to 8 MB can be attached');
        resolve();
        return;
      }
      if (this.attachments.length >= 4) {
        this.note('attach at most 4 images per message');
        resolve();
        return;
      }
      const reader = new FileReader();
      reader.onload = () => {
        const url = String(reader.result ?? '');
        const at = url.indexOf(',');
        if (at < 0) {
          resolve();
          return;
        }
        const data = url.slice(at + 1);
        this.attachments = [...this.attachments, { mime: file.type, data, url }];
        resolve();
      };
      reader.onerror = () => resolve();
      reader.readAsDataURL(file);
    });
  }

  removeAttachment(index: number): void {
    this.attachments = this.attachments.filter((_, i) => i !== index);
  }

  clearAttachments(): void {
    this.attachments = [];
  }

  get visionUnsupported(): boolean {
    return this.selectedModel?.supports_vision === false;
  }

  note(text: string): void {
    this.blocks.push({ id: nextBlockId++, kind: 'text', text, streaming: false, local: true });
  }

  /** Record time-to-first-token once per turn. */
  private markFirstToken(): void {
    if (this.firstTokenMs !== null || this.turnStartedAt === null) return;
    this.firstTokenMs = Date.now() - this.turnStartedAt;
  }

  /** Flash the taskbar when a long turn finishes while the window is in the background. */
  private async attention(): Promise<void> {
    this.turnStartedAt = null;
    if (this.dayBudget) void this.refreshToday();
    if (!inTauri()) return;
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      const win = getCurrentWindow();
      if (await win.isFocused()) return;
      this.chime();
      if (!this.prefs.notify_on_done) return;
      await win.requestUserAttention(2);
      this.notice = { id: Date.now(), text: 'Turn finished — click to return' };
    } catch {
      // attention is best-effort
    }
  }

  dismissNotice(): void {
    this.notice = null;
  }

  async focusWindow(): Promise<void> {
    this.dismissNotice();
    if (!inTauri()) return;
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().setFocus();
    } catch {
      // ignore
    }
  }

  async rewindCheckpoint(
    id: string,
    mode: 'files_only' | 'transcript_only' | 'both' = 'files_only',
  ): Promise<void> {
    if (!inTauri()) return;
    const cp = this.checkpoints.find((c) => c.id === id);
    const userIndex = cp?.userIndex ?? null;
    try {
      const { sessionRewindSplit } = await import('./ipc');
      await sessionRewindSplit(id, mode, userIndex);
      if (mode === 'transcript_only' || mode === 'both') {
        if (userIndex !== null) {
          let seen = 0;
          const cutIdx = this.blocks.findIndex((b) => {
            if (b.kind === 'user') {
              if (seen === userIndex) return true;
              seen += 1;
            }
            return false;
          });
          if (cutIdx >= 0) {
            this.blocks = this.blocks.slice(0, cutIdx);
            this.sessionTurns = userIndex;
            this.checkpoints = this.checkpoints.filter(
              (c) => (c.userIndex ?? 0) < userIndex,
            );
          }
        }
      }
      const label =
        mode === 'files_only'
          ? 'files'
          : mode === 'transcript_only'
            ? 'transcript'
            : 'files & transcript';
      this.blocks.push({
        id: nextBlockId++,
        kind: 'text',
        text: `rewound ${label} to checkpoint ${id.slice(0, 8)}`,
        streaming: false,
        local: true,
      });
      if (mode !== 'transcript_only') {
        void this.refreshIndex();
        void this.refreshChanges();
      }
    } catch (e) {
      this.blocks.push({
        id: nextBlockId++,
        kind: 'error',
        text: `rewind failed: ${e instanceof Error ? e.message : String(e)}`,
      });
    }
  }

  async loadCheckpointDiff(id: string): Promise<void> {
    if (this.checkpointDiffPreview?.id === id) {
      this.checkpointDiffPreview = null;
      return;
    }
    if (!inTauri()) return;
    try {
      const { sessionCheckpointDiff } = await import('./ipc');
      const diff = await sessionCheckpointDiff(id, null);
      this.checkpointDiffPreview = {
        id,
        diff: diff.trim() || '(no changes vs workspace)',
      };
    } catch (e) {
      this.note(`checkpoint diff failed: ${e instanceof Error ? e.message : String(e)}`);
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
    const currentValid = this.providers.some((p) => p.name === this.providerName);
    if (!currentValid) {
      const def = this.providers.find((p) => p.is_default) ?? this.providers[0];
      if (def) {
        this.providerName = def.name;
        this.provider = def.name;
        this.model = def.model;
      } else {
        this.providerName = '';
        this.provider = '';
        this.model = '';
      }
    } else if (this.providerName === 'local') {
      const localProv = this.providers.find((p) => p.name === 'local');
      const localStillHasModel = lists
        .flat()
        .some((x) => x.provider === 'local' && x.id === this.model);
      if (localProv && !localStillHasModel) {
        this.model = localProv.model;
      }
    }
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
    const prevModel = this.model;
    this.model = id;
    try {
      await m.sessionSetOptions(id, null, this.selectedModel?.context_tokens ?? null);
      await m.providerSetModel(target, id).catch(() => {});
    } catch (e) {
      this.model = prevModel;
      if (target === 'local') {
        this.blocks.push({
          id: nextBlockId++,
          kind: 'error',
          text: `couldn't load local model ${id}: ${e instanceof Error ? e.message : String(e)}`,
        });
      }
    }
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

  async enterChat(): Promise<void> {
    void this.refreshSessions();
    if (this.sessionId || !inTauri()) return;
    const saved =
      localStorage.getItem(LAST_SESSION) ??
      localStorage.getItem('hate:session:chat') ??
      localStorage.getItem('hate:session:code');
    localStorage.removeItem('hate:session:chat');
    localStorage.removeItem('hate:session:code');
    if (saved) {
      try {
        await this.openSession(saved);
        return;
      } catch {
        localStorage.removeItem(LAST_SESSION);
      }
    }
    this.clearActive();
  }

  private clearActive(): void {
    if (!inTauri()) return;
    this.sessionId = null;
    this.sessionMeta = null;
    this.resetConversation();
    this.setFolder(null);
  }

  startFresh(): void {
    if (this.running) this.cancel();
    localStorage.removeItem(LAST_SESSION);
    this.clearActive();
    if (!inTauri()) this.resetConversation();
    if (this.mode !== 'chat') this.setMode('chat');
  }

  private setFolder(path: string | null): void {
    if (path === this.workspacePath) return;
    this.workspacePath = path;
    this.closeViewer();
    this.pinned = [];
    void this.refreshIndex();
    void this.refreshWorkspaceExtras();
  }

  async openFromHistory(s: SessionMeta): Promise<void> {
    if (s.id !== this.sessionId) {
      try {
        await this.openSession(s.id);
      } catch (e) {
        this.note(`couldn't open that session: ${e instanceof Error ? e.message : String(e)}`);
        return;
      }
    }
    if (this.mode !== 'chat') this.setMode('chat');
  }

  async refreshSessions(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    if (this.sessions.length === 0) this.sessionsState = 'loading';
    try {
      this.sessions = await m.sessionsList();
      const cur = this.sessions.find((s) => s.id === this.sessionId);
      if (cur && this.sessionMeta) this.sessionMeta.title = cur.title;
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
      this.setFolder(res.meta.workspace ?? null);
      await this.syncSelection(m);
      localStorage.setItem(LAST_SESSION, id);
      void this.refreshSessions();
      void this.refreshBranches();
      void this.refreshWorkspaceExtras();
    } finally {
      this.sessionBusy = false;
    }
  }

  async renameSession(id: string, title: string): Promise<boolean> {
    if (!inTauri()) return false;
    try {
      const m = await import('./ipc');
      await m.sessionRename(id, title);
      const target = this.sessions.find((s) => s.id === id);
      if (target) target.title = title.trim();
      if (this.sessionMeta?.id === id) this.sessionMeta.title = title.trim();
      return true;
    } catch (e) {
      this.note(`rename failed: ${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
  }

  async newSession(): Promise<void> {
    if (!inTauri()) {
      this.resetConversation();
      return;
    }
    const m = await import('./ipc');
    this.sessionBusy = true;
    try {
      const meta = await m.sessionNew(this.workspacePath);
      this.sessionId = meta.id;
      this.sessionMeta = meta;
      this.resetConversation();
      await this.syncSelection(m);
      localStorage.setItem(LAST_SESSION, meta.id);
      void this.refreshSessions();
    } finally {
      this.sessionBusy = false;
    }
  }

  async deleteSession(id: string): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    await m.sessionDelete(id);
    this.sessions = this.sessions.filter((s) => s.id !== id);
    if (localStorage.getItem(LAST_SESSION) === id) localStorage.removeItem(LAST_SESSION);
    if (this.sessionId === id) this.clearActive();
  }

  private resetConversation(): void {
    this.blocks = [];
    this.branchGroups = [];
    this.contexts = [];
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
    this.agentCtxSize = 0;
    this.costUsd = 0;
    this.cacheRatio = 0;
    this.tokensSaved = 0;
    this.cacheBreakReason = null;
    this.toolAudits = [];
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
    this.agentCtxSize = 0;
    this.roundStart = 0;
    this.pendingToolUsage = null;
    this.turnBlockId = null;
    this.planReady = false;
  }

  settingsSection = $state('appearance');

  openSettings(section?: string): void {
    if (section) this.settingsSection = section;
    this.setMode('settings');
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
    void this.savePrefs({ theme });
  }

  /** Experimental: frosted, floating glass panes over an ambient light field. */
  glass = $state(false);

  setGlass(on: boolean): void {
    this.glass = on;
    localStorage.setItem('hate:glass', on ? 'on' : 'off');
    this.applyTheme();
    void this.savePrefs({ glass: on });
  }

  private applyTheme(): void {
    if (typeof document === 'undefined') return;
    if (this.glass) document.documentElement.dataset.glass = '';
    else document.documentElement.removeAttribute('data-glass');
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
    if (p) await this.attachFolder(p);
  }

  async attachFolder(path: string | null): Promise<void> {
    if (!inTauri() || path === this.workspacePath) return;
    if (this.running) {
      this.note('wait for the reply to finish before changing the folder');
      return;
    }
    if (this.sessionId) {
      const m = await import('./ipc');
      try {
        await m.sessionSetWorkspace(path);
      } catch (e) {
        this.note(`couldn't change the folder: ${e instanceof Error ? e.message : String(e)}`);
        return;
      }
      if (this.sessionMeta) this.sessionMeta.workspace = path;
      const row = this.sessions.find((s) => s.id === this.sessionId);
      if (row) row.workspace = path;
      if (this.blocks.length > 0) {
        this.note(
          path
            ? 'folder attached — hate-agent can use it from your next message'
            : 'folder removed — the next message has no file access',
        );
      }
    }
    this.setFolder(path);
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

  async refreshWorkspaceExtras(): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    this.customCommands = await m.customCommandsList().catch(() => []);
    if (!this.workspacePath) {
      this.worktree = null;
      this.workspaceRules = [];
      this.repoTrust = null;
      this.trustDismissed = false;
      return;
    }
    const [wt, rules, trust] = await Promise.all([
      m.workspaceWorktreeStatus().catch(() => null),
      m.workspaceRulesInfo().catch(() => []),
      m.workspaceTrustStatus().catch(() => null),
    ]);
    this.worktree = wt;
    this.workspaceRules = rules;
    if (this.repoTrust?.canonical_root !== trust?.canonical_root) {
      this.trustDismissed = false;
    }
    this.repoTrust = trust;
  }

  async approveRepoTrust(trust: boolean): Promise<void> {
    if (!inTauri()) return;
    const m = await import('./ipc');
    try {
      const updated = await m.workspaceTrustApprove(trust);
      this.repoTrust = updated;
      this.trustDismissed = !trust;
      this.note(trust ? 'workspace trusted — project rules enabled' : 'workspace kept restricted');
    } catch (e) {
      this.note(`trust update failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async enterWorktree(branch?: string | null): Promise<void> {
    if (!inTauri() || !this.workspacePath) return;
    if (this.running) {
      this.note('wait for the reply to finish before creating a worktree');
      return;
    }
    await this.ensureSession();
    const m = await import('./ipc');
    try {
      const wtPath = await m.workspaceWorktreeCreate(branch ?? null);
      if (this.sessionMeta) this.sessionMeta.workspace = wtPath;
      const row = this.sessions.find((s) => s.id === this.sessionId);
      if (row) row.workspace = wtPath;
      this.setFolder(wtPath);
      this.setNotice('Isolated git worktree ready');
    } catch (e) {
      this.note(`couldn't create worktree: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async mergeBackWorktree(): Promise<void> {
    if (!inTauri() || !this.worktree?.is_worktree) return;
    if (this.running) {
      this.note('wait for the reply to finish before merging');
      return;
    }
    const m = await import('./ipc');
    try {
      const res = await m.workspaceWorktreeMergeBack();
      this.setNotice(`Merged ${res.merged_branch} → ${res.target_branch}`);
      await this.refreshWorkspaceExtras();
    } catch (e) {
      this.note(`merge back failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async discardWorktree(force = false): Promise<void> {
    if (!inTauri() || !this.worktree?.is_worktree) return;
    if (this.running) {
      this.note('wait for the reply to finish before removing the worktree');
      return;
    }
    const m = await import('./ipc');
    try {
      const mainRoot = await m.workspaceWorktreeRemove(force);
      if (this.sessionMeta) this.sessionMeta.workspace = mainRoot;
      const row = this.sessions.find((s) => s.id === this.sessionId);
      if (row) row.workspace = mainRoot;
      this.setFolder(mainRoot);
      this.setNotice('Returned to main repository');
    } catch (e) {
      this.note(`couldn't remove worktree: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async spawnWorktreeTrack(
    name = `track-${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`,
    model?: string,
    provider?: string,
    openNow = true,
  ): Promise<{ session_id: string; worktree_path: string; branch: string } | null> {
    if (!inTauri() || !this.workspacePath) return null;
    const m = await import('./ipc');
    try {
      const dto = await m.sessionSpawnWorktree(
        name,
        this.workspacePath,
        model ?? (this.model || null),
        provider ?? (this.providerName || this.provider || null),
      );
      await this.refreshSessions();
      if (openNow && !this.running) {
        await this.openSession(dto.session_id);
        if (this.mode !== 'chat') this.setMode('chat');
      }
      this.setNotice(`Worktree track ${dto.branch} ready`);
      return dto;
    } catch (e) {
      this.note(`couldn't spawn track: ${e instanceof Error ? e.message : String(e)}`);
      return null;
    }
  }

  async compareModelsInWorktrees(
    prompt: string,
    targets: Array<{ provider: string; model: string }>,
  ): Promise<void> {
    if (!inTauri() || !this.workspacePath || targets.length === 0) return;
    const trimmed = prompt.trim();
    if (!trimmed) {
      this.note('enter a prompt to compare models');
      return;
    }
    const created: Array<{ session_id: string; worktree_path: string; branch: string }> = [];
    for (const t of targets) {
      const dto = await this.spawnWorktreeTrack(t.model, t.model, t.provider, false);
      if (dto) created.push(dto);
    }
    if (created.length === 0) return;
    await this.refreshSessions();
    if (!this.running && created[0]) {
      await this.openSession(created[0].session_id);
      if (this.mode !== 'chat') this.setMode('chat');
      this.draft = trimmed;
      this.setNotice(
        `Created ${created.length} worktree tracks — switch between them in the sidebar`,
      );
    }
  }

  async createRulesFile(file = '.hate-agent.md'): Promise<void> {
    if (!inTauri() || !this.workspacePath) return;
    const m = await import('./ipc');
    try {
      const created = await m.workspaceCreateRules(file);
      await this.refreshWorkspaceExtras();
      await this.refreshIndex();
      await this.openFile(created);
    } catch (e) {
      this.note(`couldn't create rules file: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  runCustomCommand(cmd: CustomCommandInfo, args = ''): void {
    const hasArgPlaceholder = cmd.body.includes('$ARGUMENTS');
    const expanded = cmd.body.replace(/\$ARGUMENTS/g, args).trim();
    if (hasArgPlaceholder && !args.trim()) {
      this.draft = expanded;
      const ta = document.querySelector<HTMLTextAreaElement>('.composer textarea');
      ta?.focus();
      return;
    }
    this.send(expanded);
  }

  async saveCustomCommand(name: string, scope: 'workspace' | 'global', content: string): Promise<boolean> {
    if (!inTauri()) return false;
    const m = await import('./ipc');
    try {
      await m.customCommandSave(name, scope, content);
      await this.refreshWorkspaceExtras();
      this.setNotice(`Saved command /${name.replace(/^\//, '')}`);
      return true;
    } catch (e) {
      this.note(`couldn't save command: ${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
  }

  handleEvent(e: SessionEvent): void {
    switch (e.type) {
      case 'delta':
        this.sessionState = 'running';
        this.markFirstToken();
        this.appendText(e.text);
        break;
      case 'thinking_delta':
        this.markFirstToken();
        this.appendThinking(e.text);
        break;
      case 'tool_started':
        this.toolsThisTurn += 1;
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
      case 'image': {
        const b = e.call_id ? this.findTool(e.call_id) : undefined;
        if (b) {
          if (!b.images?.includes(e.source)) b.images = [...(b.images ?? []), e.source];
        } else {
          this.blocks.push({ id: nextBlockId++, kind: 'image', images: [e.source] });
        }
        break;
      }
      case 'tool_finished': {
        const b = this.findTool(e.call_id);
        if (b) {
          b.ok = e.ok;
          b.fullTokens = e.full_tokens;
          b.sentTokens = e.sent_tokens;
        }
        const saved = Math.max(0, e.full_tokens - e.sent_tokens);
        if (saved > 0) {
          this.tokensSaved += saved;
        }
        const audit: ToolAuditEntry = {
          callId: e.call_id,
          name: b?.call?.name ?? 'tool',
          ok: e.ok,
          fullTokens: e.full_tokens,
          sentTokens: e.sent_tokens,
          fullPreview: e.full_preview ?? null,
          sentPreview: e.sent_preview ?? b?.preview ?? null,
        };
        this.toolAudits = [audit, ...this.toolAudits].slice(0, 12);
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
        if (e.cache_break_reason) {
          this.cacheBreakReason = e.cache_break_reason;
        }
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
      case 'checkpoint_created': {
        const userCount = this.blocks.filter((b) => b.kind === 'user').length;
        this.checkpoints = [
          {
            id: e.id,
            label: 'checkpoint',
            at: new Date().toLocaleTimeString(),
            userIndex: userCount > 0 ? userCount - 1 : 0,
          },
          ...this.checkpoints,
        ];
        break;
      }
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
        if (e.outcome.cache_break_reason) {
          this.cacheBreakReason = e.outcome.cache_break_reason;
        }
        this.recomputeDerived();
        this.sessionState = 'done';
        this.finishStreamingBlocks();
        void this.refreshIndex();
        this.finishTurn(e.outcome);
        void this.attention();
        setTimeout(() => void this.refreshSessions(), 600);
        break;
      case 'context_usage':
        this.ctxUsed = e.used;
        this.agentCtxSize = e.size;
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
      void this.refreshWorkspaceExtras();
      if (this.viewer) void this.refreshViewer();
    }
    if (completed && !this.planReady && this.queue.length > 0) {
      const next = this.queue.shift();
      if (next) {
        // Carry queued attachments back through the composer slot so the
        // normal send path picks them up.
        this.attachments = next.images.map((a) => ({ ...a, url: `data:${a.mime};base64,${a.data}` }));
        queueMicrotask(() => this.send(next.text, next.pins, next.contexts));
      }
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
