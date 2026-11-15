<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type { UsageTotalsInfo } from '../ipc';

  const RANGES = [7, 30, 90] as const;

  onMount(() => {
    if (!session.usageOverview) void session.loadUsage(session.usageDays);
  });

  const totals = $derived(session.usageOverview?.totals ?? null);
  const days = $derived(session.usageOverview?.by_day ?? []);
  const models = $derived(session.usageOverview?.by_model ?? []);
  const sessions = $derived(session.usageOverview?.sessions ?? []);

  const maxDayCost = $derived(Math.max(0.0001, ...days.map((d) => d.totals.cost_usd)));
  const maxModelCost = $derived(Math.max(0.0001, ...models.map((m) => m.totals.cost_usd)));

  function money(v: number): string {
    if (v <= 0) return '$0';
    if (v >= 10) return `$${v.toFixed(2)}`;
    if (v >= 1) return `$${v.toFixed(2)}`;
    return `$${v.toFixed(4)}`;
  }

  function k(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }

  function cachePct(t: UsageTotalsInfo | null): string {
    if (!t || t.input_tokens === 0) return '—';
    return `${Math.round((t.cached_input_tokens / t.input_tokens) * 100)}%`;
  }

  function median(vals: number[]): number {
    if (vals.length === 0) return 0;
    const s = [...vals].sort((a, b) => a - b);
    const mid = Math.floor(s.length / 2);
    return s.length % 2 ? (s[mid] ?? 0) : ((s[mid - 1] ?? 0) + (s[mid] ?? 0)) / 2;
  }

  const p50Latency = $derived(
    median(
      models
        .filter((m) => m.totals.requests > 0)
        .map((m) => m.totals.latency_ms / m.totals.requests),
    ),
  );

  function dayLabel(epoch: number): string {
    return new Date(epoch * 1000).toLocaleDateString(undefined, {
      month: 'short',
      day: 'numeric',
    });
  }

  function when(epoch: number): string {
    const diff = Date.now() / 1000 - epoch;
    if (diff < 3600) return `${Math.max(1, Math.round(diff / 60))}m ago`;
    if (diff < 86_400) return `${Math.round(diff / 3600)}h ago`;
    return `${Math.round(diff / 86_400)}d ago`;
  }
</script>

<div class="usage">
  <div class="range-row">
    <div class="range" role="group" aria-label="Usage range">
      {#each RANGES as r}
        <button
          class="range-btn mono"
          class:active={session.usageDays === r}
          onclick={() => void session.loadUsage(r)}
          aria-pressed={session.usageDays === r}
        >
          {r}d
        </button>
      {/each}
    </div>
    <span class="spacer"></span>
    {#if session.usageState === 'loading'}
      <span class="mono dim">loading…</span>
    {:else if totals && totals.requests > 0}
      <span class="mono dim">{totals.requests} requests</span>
    {/if}
    <button
      class="icon-btn"
      onclick={() => void session.loadUsage(session.usageDays)}
      aria-label="Refresh usage"
      title="Refresh"
    >
      <Icon name="refresh" size={11} />
    </button>
  </div>

  {#if session.usageState === 'error'}
    <p class="empty">{session.usageError}</p>
  {:else if !totals || totals.requests === 0}
    <div class="empty-block">
      <p class="empty-title">No usage recorded yet</p>
      <p class="empty-body">
        Every request is written to a local SQLite ledger with its tokens, latency and
        price. Run a turn and this fills in — nothing leaves your machine.
      </p>
    </div>
  {:else}
    <div class="stats">
      <div class="stat">
        <span class="stat-label mono">spend</span>
        <span class="stat-value">{money(totals.cost_usd)}</span>
        <span class="stat-sub mono">{money(totals.cost_usd / session.usageDays)}/day avg</span>
      </div>
      <div class="stat">
        <span class="stat-label mono">cache hit</span>
        <span class="stat-value">{cachePct(totals)}</span>
        <span class="stat-sub mono">{k(totals.cached_input_tokens)} cached in</span>
      </div>
      <div class="stat">
        <span class="stat-label mono">tokens</span>
        <span class="stat-value">{k(totals.input_tokens + totals.output_tokens)}</span>
        <span class="stat-sub mono">{k(totals.output_tokens)} out · {k(totals.thinking_tokens)} think</span>
      </div>
      <div class="stat">
        <span class="stat-label mono">latency</span>
        <span class="stat-value">{p50Latency > 0 ? `${(p50Latency / 1000).toFixed(1)}s` : '—'}</span>
        <span class="stat-sub mono">mean per request</span>
      </div>
    </div>

    {#if days.length > 0}
      <p class="sub-title mono">by day</p>
      <div class="bars" aria-hidden="true">
        {#each days as d (d.day)}
          <span
            class="bar"
            class:today={dayLabel(d.day) === dayLabel(Math.floor(Date.now() / 1000))}
            style:height={`${Math.max(3, Math.round((d.totals.cost_usd / maxDayCost) * 46))}px`}
            title={`${dayLabel(d.day)} — ${money(d.totals.cost_usd)} · ${d.totals.requests} req`}
          ></span>
        {/each}
      </div>
      <div class="axis mono">
        <span>{dayLabel(days[0]?.day ?? 0)}</span>
        {#if days.length > 1}
          <span>{dayLabel(days[days.length - 1]?.day ?? 0)}</span>
        {/if}
      </div>
    {/if}

    <p class="sub-title mono">by model</p>
    <ul class="model-list">
      {#each models.slice(0, 6) as m (m.model + m.provider)}
        <li class="model-row">
          <span class="model-name mono" title={`${m.provider} / ${m.model}`}>{m.model}</span>
          <span class="model-bar" aria-hidden="true">
            <span style:width={`${Math.round((m.totals.cost_usd / maxModelCost) * 100)}%`}></span>
          </span>
          <span class="model-cost mono">{money(m.totals.cost_usd)}</span>
          <span class="model-meta mono">{k(m.totals.input_tokens)} in · {cachePct(m.totals)} cached</span>
        </li>
      {/each}
    </ul>

    {#if sessions.length > 0}
      <p class="sub-title mono">recent sessions</p>
      <ul class="session-list">
        {#each sessions.slice(0, 6) as s (s.session_id)}
          <li class="session-row">
            <span class="session-title" title={s.title}>{s.title}</span>
            <span class="session-when mono">{when(s.started_at)}</span>
            <span class="session-cost mono">{money(s.totals.cost_usd)}</span>
          </li>
        {/each}
      </ul>
    {/if}

    {#if !session.usageOverview?.priced}
      <p class="note">
        The bundled price table is older than 90 days — spend for newer models may read
        low until it's refreshed.
      </p>
    {/if}
  {/if}
</div>

<style>
  .usage {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .range-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }
  .range {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .range-btn {
    padding: 2px 8px;
    border-radius: 6px;
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .range-btn.active {
    background: var(--bg-0);
    color: var(--fg-0);
  }
  .spacer {
    flex: 1 1 auto;
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 20px;
    border-radius: 5px;
    color: var(--fg-3);
  }
  .icon-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .dim {
    color: var(--fg-3);
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: 8px;
    margin-bottom: 6px;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 12px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: 12px;
  }
  .stat-label {
    font-size: 9.5px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .stat-value {
    font-size: 17px;
    font-weight: 600;
    color: var(--fg-0);
    line-height: 1.15;
  }
  .stat-sub {
    font-size: 9.5px;
    color: var(--fg-3);
  }
  .sub-title {
    margin: 16px 0 6px;
    font-size: 9.5px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .bars {
    display: flex;
    align-items: flex-end;
    gap: 3px;
    height: 50px;
    padding: 2px 0;
  }
  .bar {
    flex: 1 1 0;
    min-width: 2px;
    max-width: 28px;
    background: var(--accent-dim);
    border-radius: 2px 2px 0 0;
    opacity: 0.75;
  }
  .bar.today {
    background: var(--accent);
    opacity: 1;
  }
  .axis {
    display: flex;
    justify-content: space-between;
    font-size: 9.5px;
    color: var(--fg-3);
    margin-top: 2px;
  }
  .model-list,
  .session-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .model-row {
    display: grid;
    grid-template-columns: minmax(0, 1.4fr) minmax(40px, 1fr) auto;
    grid-template-areas:
      'name bar cost'
      'meta meta meta';
    align-items: center;
    gap: 4px 10px;
    font-size: 11px;
  }
  .model-name {
    grid-area: name;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-1);
  }
  .model-bar {
    grid-area: bar;
    height: 3px;
    border-radius: 999px;
    background: var(--bg-3);
    overflow: hidden;
  }
  .model-bar > span {
    display: block;
    height: 100%;
    background: var(--accent-dim);
  }
  .model-cost {
    grid-area: cost;
    color: var(--fg-1);
    text-align: right;
  }
  .model-meta {
    grid-area: meta;
    font-size: 9.5px;
    color: var(--fg-3);
  }
  .session-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 10px;
    align-items: baseline;
    font-size: 11px;
    padding: 4px 0;
    border-bottom: 1px solid var(--border);
  }
  .session-row:last-child {
    border-bottom: none;
  }
  .session-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-1);
  }
  .session-when,
  .session-cost {
    color: var(--fg-3);
    font-size: 10px;
  }
  .note {
    margin: 12px 0 0;
    font-size: 10.5px;
    color: var(--warn);
  }
  .empty {
    margin: 8px 0;
    font-size: 11.5px;
    color: var(--fg-3);
  }
  .empty-block {
    padding: 14px 16px;
    background: var(--bg-0);
    border: 1px dashed var(--border);
    border-radius: 12px;
  }
  .empty-title {
    margin: 0 0 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-1);
  }
  .empty-body {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.55;
    color: var(--fg-3);
    max-width: 60ch;
  }
</style>
