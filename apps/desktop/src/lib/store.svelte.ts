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
  PermissionRequest,
  PlanStep,
  SessionEvent,
  SymbolHit,
  TokenUsage,
  TranscriptBlock,
} from './types';

const inTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

let nextBlockId = 2;

class SessionStore {
  // ── transcript ──────────────────────────────────────────────────────────
  blocks = $state<TranscriptBlock[]>([...fixtureInitialBlocks]);

  // ── left pane ───────────────────────────────────────────────────────────
  tree = $state(fixtureTree);
  symbols = $state<SymbolHit[]>(fixtureSymbols);
  selectedPath = $state<string | null>(null);

  // ── right pane ──────────────────────────────────────────────────────────
  plan = $state<PlanStep[]>([...fixturePlan]);
  usage = $state<TokenUsage>({ ...fixtureUsage });
  budgetTokens = 200_000;
  checkpoints = $state<CheckpointEntry[]>([...fixtureCheckpoints]);
  changedFiles = $state<ChangedFile[]>([...fixtureChangedFiles]);
  model = $state('nous/hermes-4-405b');
  provider = $state('nous');
  sessionState = $state<'idle' | 'running' | 'awaiting_permission' | 'awaiting_user' | 'done' | 'failed'>(
    'idle',
  );
  costUsd = $state(0.0031);
  cacheRatio = $state(0.82);

  // ── composer / permission ───────────────────────────────────────────────
  draft = $state('');
  pendingPermission = $state<PermissionRequest | null>(null);
  awaitingPrompt = $state<string | null>(null);
  thinkingLevel = $state<0 | 1 | 2>(1);

  // ── panes ───────────────────────────────────────────────────────────────
  leftCollapsed = $state(false);
  rightCollapsed = $state(false);
  leftWidth = $state(260);
  rightWidth = $state(300);

  // ── internals ───────────────────────────────────────────────────────────
  private unlisten: UnlistenFn | null = null;
  private fixtureTimers: ReturnType<typeof setTimeout>[] = [];

  get running(): boolean {
    return this.sessionState === 'running';
  }

  /** Wire the event stream. Called once on app mount. */
  async start(): Promise<void> {
    if (!inTauri()) {
      this.startFixturePlayback();
      return;
    }
    const { listen } = await import('@tauri-apps/api/event');
    this.unlisten = await listen<SessionEvent>('core-event', (e) => {
      this.handleEvent(e.payload);
    });
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
        this.appendText(`\n\nerror: ${String(err)}`);
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

  cycleThinking(): void {
    this.thinkingLevel = ((this.thinkingLevel + 1) % 3) as 0 | 1 | 2;
  }

  clearTranscript(): void {
    this.blocks = [];
    nextBlockId = 1;
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
        this.sessionState = 'failed';
        this.finishStreamingBlocks();
        break;
      case 'error':
        this.appendText(`\n\nerror: ${e.error.message ?? 'unknown'}`);
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
