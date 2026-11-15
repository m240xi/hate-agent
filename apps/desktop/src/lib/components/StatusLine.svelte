<script lang="ts">
  import { session } from '../store.svelte';

  const ctxPct = $derived(
    session.agentCtxSize > 0 ? Math.min(1, session.ctxUsed / session.agentCtxSize) : 0,
  );
  const budgetPct = $derived(session.sessionBudget ? Math.min(1, session.sessionCostRatio) : null);

  function fmtCost(v: number): string {
    if (v <= 0) return '$0';
    return v >= 1 ? `$${v.toFixed(2)}` : `$${v.toFixed(4)}`;
  }

  function fmtTok(n: number): string {
    return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
  }

  const openActivity = () => session.openActivity();
</script>

<div class="line" aria-label="Session status">
  {#if session.sessionTurns === 0 && session.usage.input === 0}
    <span>hate-agent shows every token it spends. Check important changes.</span>
  {:else}
    <button
      onclick={openActivity}
      title={`context ${session.ctxUsed.toLocaleString()} of ${session.agentCtxSize.toLocaleString()} tokens`}
    >
      <span class="bar" class:hot={ctxPct >= 0.8}>
        <span style:width={`${Math.round(ctxPct * 100)}%`}></span>
      </span>
      <span class:hot={ctxPct >= 0.8}>{Math.round(ctxPct * 100)}% context</span>
    </button>
    <span class="sep">·</span>
    <button
      onclick={() => session.openSettings('spending')}
      title={budgetPct === null ? 'Set a spending ceiling in Settings' : `${Math.round(budgetPct * 100)}% of the session ceiling`}
    >
      <span class:hot={budgetPct !== null && budgetPct >= 0.8}>
        {fmtCost(session.costUsd)}{session.sessionBudget ? ` of ${fmtCost(session.sessionBudget)}` : ''}
      </span>
    </button>
    {#if session.usage.input > 0}
      <span class="sep">·</span>
      <button onclick={openActivity} title="input / output tokens this session">
        {fmtTok(session.sessionTotals.input || session.usage.input)} in ·
        {fmtTok(session.sessionTotals.output || session.usage.output)} out
      </button>
    {/if}
  {/if}
</div>

<style>
  .line {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-wrap: wrap;
    gap: 6px;
    min-height: 22px;
    padding: 4px 0 8px;
    font-size: 11.5px;
    color: var(--fg-3);
    user-select: none;
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 1px 6px;
    border-radius: 6px;
    color: var(--fg-3);
  }
  button:hover {
    background: var(--bg-2);
    color: var(--fg-1);
  }
  .sep {
    opacity: 0.6;
  }
  .hot {
    color: var(--warn);
  }
  .bar {
    width: 28px;
    height: 3px;
    border-radius: 999px;
    background: var(--bg-3);
    overflow: hidden;
  }
  .bar > span {
    display: block;
    height: 100%;
    background: var(--fg-2);
  }
  .bar.hot > span {
    background: var(--warn);
  }
</style>
