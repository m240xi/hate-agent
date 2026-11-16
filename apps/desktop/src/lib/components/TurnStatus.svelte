<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  let now = $state(Date.now());

  $effect(() => {
    if (!session.running) return;
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  const elapsed = $derived(
    session.turnStartedAt === null ? 0 : Math.max(0, now - session.turnStartedAt),
  );

  function fmt(ms: number): string {
    const s = Math.floor(ms / 1000);
    if (s < 60) return `0:${String(s).padStart(2, '0')}`;
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }

  const firstToken = $derived(
    session.firstTokenMs === null ? 'waiting for first token' : `first token ${fmt(session.firstTokenMs)}`,
  );

  const ctxPct = $derived(
    session.agentCtxSize > 0
      ? Math.min(1, session.ctxUsed / session.agentCtxSize)
      : 0,
  );

  const budget = $derived.by(() => {
    const limit = session.sessionBudget;
    if (!limit) return null;
    return {
      limit,
      pct: Math.min(1, session.sessionCostRatio),
      remaining: Math.max(0, limit - session.costUsd),
    };
  });

  const dayBudget = $derived.by(() => {
    const limit = session.dayBudget;
    if (!limit) return null;
    return { limit, pct: Math.min(1, session.dayCostRatio) };
  });
</script>

{#if session.budgetPrompt}
  <div class="budget-bar" role="alert">
    <Icon name="alert" size={12} />
    <span>
      Spending ceiling reached —
      {#if session.sessionBudget && session.sessionCostRatio >= 1}
        ${session.costUsd.toFixed(2)} of ${session.sessionBudget.toFixed(2)} this session
      {:else if dayBudget}
        ${session.todayCostUsd.toFixed(2)} of ${dayBudget.limit.toFixed(2)} today
      {/if}
    </span>
    <span class="spacer"></span>
    <button class="btn ghost sm" onclick={() => session.dismissBudgetPrompt()}>Hold off</button>
    <button class="btn sm" onclick={() => session.continueOverBudget()}>Continue anyway</button>
  </div>
{:else if session.budgetWarning && !session.running}
  <div class="budget-bar warn" role="status">
    <Icon name="alert" size={12} />
    <span>
      {Math.round(Math.max(session.sessionCostRatio, session.dayCostRatio) * 100)}% of your
      spending ceiling used
    </span>
    <span class="spacer"></span>
    <button class="btn ghost sm" onclick={() => session.openSettings('spending')}>Raise limit</button>
  </div>
{/if}

{#if session.running || session.pendingPermission}
  <div class="turn-status" role="status" aria-live="off">
    <span class="pulse" aria-hidden="true"></span>
    <span class="label mono">{session.pendingPermission ? 'needs approval' : 'working'}</span>
    <span class="dot">·</span>
    <span class="mono">{fmt(elapsed)}</span>
    <span class="dot">·</span>
    <span class="mono dim">{firstToken}</span>
    {#if session.toolsThisTurn > 0}
      <span class="dot">·</span>
      <span class="mono dim">{session.toolsThisTurn} tool{session.toolsThisTurn === 1 ? '' : 's'}</span>
    {/if}
    {#if ctxPct >= 0.75}
      <span class="dot">·</span>
      <span class="mono ctx-hot" title="context window filling up">
        ctx {Math.round(ctxPct * 100)}%
      </span>
    {/if}
    {#if session.costUsd > 0}
      <span class="dot">·</span>
      <span class="mono dim">
        ${session.costUsd.toFixed(4)}{#if budget}
          / ${budget.limit.toFixed(2)}{/if}
      </span>
    {/if}
    <span class="spacer"></span>
    {#if budget}
      <span
        class="meter"
        class:hot={budget.pct >= 0.8}
        title={`${Math.round(budget.pct * 100)}% of this session's ceiling · $${budget.remaining.toFixed(2)} left`}
      >
        <span style:width={`${Math.round(budget.pct * 100)}%`}></span>
      </span>
    {/if}
  </div>
{/if}

{#if !session.running && ctxPct >= 0.8}
  <div class="ctx-nudge" role="status">
    <span class="mono">context {Math.round(ctxPct * 100)}% full</span>
    <button class="btn ghost sm" onclick={() => void session.compact()}>Compact now</button>
  </div>
{/if}

<style>
  .turn-status,
  .budget-bar,
  .ctx-nudge {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0 20px 2px;
    padding: 5px 10px;
    font-size: 11px;
    color: var(--fg-2);
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    align-self: flex-start;
    max-width: 100%;
    overflow: hidden;
  }
  .budget-bar {
    color: var(--fg-1);
    border-color: color-mix(in srgb, var(--err) 40%, var(--border));
  }
  .budget-bar.warn {
    border-color: color-mix(in srgb, var(--warn) 45%, var(--border));
  }
  .budget-bar .spacer,
  .turn-status .spacer {
    flex: 1 1 auto;
  }
  .ctx-nudge {
    gap: 10px;
    color: var(--fg-3);
    border-style: dashed;
  }
  .label {
    color: var(--fg-1);
  }
  .dot {
    color: var(--fg-3);
  }
  .dim {
    color: var(--fg-3);
  }
  .ctx-hot {
    color: var(--warn);
  }
  /* a 2-frame block cursor: the pixel-art equivalent of a spinner */
  .pulse {
    width: 6px;
    height: 8px;
    border-radius: 1px;
    background: var(--accent);
    box-shadow: 0 0 8px -1px color-mix(in srgb, var(--accent) 70%, transparent);
    animation: status-blip 1s steps(2, end) infinite;
  }
  @keyframes status-blip {
    50% {
      opacity: 0.25;
    }
  }
  .meter {
    flex: none;
    width: 54px;
    height: 3px;
    border-radius: 999px;
    background: var(--bg-3);
    overflow: hidden;
  }
  .meter > span {
    display: block;
    height: 100%;
    background: var(--accent-dim);
    transition: width var(--motion);
  }
  .meter.hot > span {
    background: var(--warn);
  }
  @keyframes status-pulse {
    50% {
      opacity: 0.25;
      transform: scale(0.85);
    }
  }
</style>
