<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  const ctxPct = $derived(
    session.agentCtxSize > 0 ? Math.min(1, session.ctxUsed / session.agentCtxSize) : 0,
  );

  const budgetPct = $derived(
    session.sessionBudget ? Math.min(1, session.sessionCostRatio) : null,
  );

  const stateLabel = $derived(
    session.sessionState === 'idle' ? 'idle' : session.sessionState.replace('_', ' '),
  );

  function fmtCost(v: number): string {
    if (v <= 0) return '$0';
    if (v >= 1) return `$${v.toFixed(2)}`;
    return `$${v.toFixed(4)}`;
  }
</script>

<footer class="status-bar" aria-label="Session status">
  <span class="state" class:live={session.running} class:err={session.sessionState === 'failed'}>
    <span class="dot" aria-hidden="true"></span>
    {stateLabel}
  </span>

  <button
    class="seg model"
    onclick={() => session.setMode('settings')}
    title={`${session.providerName || session.provider} · click to manage providers`}
  >
    <Icon name="bolt" size={10} />
    <span class="mono">{session.providerName || session.provider || 'no provider'}</span>
    <span class="sep">/</span>
    <span class="mono dim">{session.model || 'no model'}</span>
  </button>

  <button
    class="seg"
    onclick={() => (session.rightCollapsed = false)}
    title={`context ${session.ctxUsed.toLocaleString()} of ${session.agentCtxSize.toLocaleString()} tokens`}
  >
    <span class="mono dim">ctx</span>
    <span class="mini" class:hot={ctxPct >= 0.8}>
      <span style:width={`${Math.round(ctxPct * 100)}%`}></span>
    </span>
    <span class="mono" class:hot={ctxPct >= 0.8}>{Math.round(ctxPct * 100)}%</span>
  </button>

  <button
    class="seg"
    onclick={() => session.setMode('settings')}
    title={
      budgetPct === null
        ? 'session spend — set a ceiling in Settings → Spending'
        : `${Math.round(budgetPct * 100)}% of the session ceiling`
    }
  >
    <span class="mono dim">spend</span>
    <span class="mono" class:hot={budgetPct !== null && budgetPct >= 0.8}>
      {fmtCost(session.costUsd)}{#if session.sessionBudget}
        <span class="dim">/{fmtCost(session.sessionBudget)}</span>
      {/if}
    </span>
  </button>

  <span class="seg static mono dim" title="turns this session">
    {session.sessionTurns} turn{session.sessionTurns === 1 ? '' : 's'}
  </span>

  <span class="spacer"></span>

  {#if session.usage.input > 0}
    <span class="seg static mono dim" title="input / output tokens this session">
      {session.usage.input.toLocaleString()} in · {session.usage.output.toLocaleString()} out
    </span>
  {/if}

  <button
    class="seg density"
    onclick={() =>
      session.setDensity(session.density === 'compact' ? 'comfortable' : 'compact')}
    title={`Transcript density: ${session.density} — click to switch`}
    aria-label="Toggle transcript density"
  >
    <Icon name={session.density === 'compact' ? 'list' : 'sliders'} size={10} />
    <span class="mono dim">{session.density === 'compact' ? 'compact' : 'cosy'}</span>
  </button>
</footer>

<style>
  .status-bar {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 24px;
    padding: 0 8px;
    flex: none;
    font-size: 10.5px;
    color: var(--fg-2);
    background: var(--bg-1);
    border-top: 1px solid var(--border);
    user-select: none;
    overflow: hidden;
  }
  .spacer {
    flex: 1 1 auto;
  }
  .state {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0 8px 0 2px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-3);
  }
  .state .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--fg-3);
  }
  .state.live {
    color: var(--fg-1);
  }
  .state.live .dot {
    background: var(--accent);
    animation: status-pulse 1.4s ease-in-out infinite;
  }
  .state.err {
    color: var(--err);
  }
  .state.err .dot {
    background: var(--err);
  }
  .seg {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 18px;
    padding: 0 7px;
    border-radius: 5px;
    color: var(--fg-2);
    white-space: nowrap;
    max-width: 100%;
    overflow: hidden;
  }
  button.seg:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .seg.static {
    cursor: default;
  }
  .sep {
    color: var(--fg-3);
  }
  .dim {
    color: var(--fg-3);
  }
  .hot {
    color: var(--warn);
  }
  .mini {
    width: 34px;
    height: 3px;
    border-radius: 999px;
    background: var(--bg-3);
    overflow: hidden;
  }
  .mini > span {
    display: block;
    height: 100%;
    background: var(--accent-dim);
  }
  .mini.hot > span {
    background: var(--warn);
  }
  .density {
    gap: 4px;
  }
  @keyframes status-pulse {
    50% {
      opacity: 0.3;
      transform: scale(0.8);
    }
  }
  @media (max-width: 720px) {
    .seg.static:last-of-type,
    .seg:nth-of-type(5) {
      display: none;
    }
  }
</style>
