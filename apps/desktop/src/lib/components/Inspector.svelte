<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }

  function fmtCtx(n: number | null | undefined): string {
    if (n == null) return '—';
    if (n >= 1_000_000) {
      const m = n / 1_000_000;
      return `${m >= 10 ? Math.round(m) : m.toFixed(1)}M`;
    }
    if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
    return String(n);
  }

  // Session clock — ticks only while a session is live.
  let now = $state(Date.now());
  $effect(() => {
    if (session.sessionBeganAt === null) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  const elapsed = $derived(
    session.sessionBeganAt === null ? '—' : fmtElapsed(now - session.sessionBeganAt),
  );

  function fmtElapsed(ms: number): string {
    const s = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    const mm = String(m).padStart(2, '0');
    const ss = String(sec).padStart(2, '0');
    return h > 0 ? `${h}:${mm}:${ss}` : `${m}:${ss}`;
  }

  const sessionTokTotal = $derived(
    session.sessionTotals.input + session.sessionTotals.output,
  );

  // Task inspector: plan steps, live token/cost meter against the model's
  // real context window, session stats, checkpoints, changed files.
  // Pure consumer of SessionStore.
</script>

<div class="inspector">
  <section class="sec">
    <h3 class="sec-title"><Icon name="list" size={11} /> Plan</h3>
    {#if session.plan.length === 0}
      <p class="dim">No plan yet — the agent emits one in plan mode.</p>
    {:else}
      <ol class="steps">
        {#each session.plan as step, i}
          <li class="step" class:done={step.status === 'done'} class:active={step.status === 'active'}>
            <span class="marker">
              {#if step.status === 'done'}<Icon name="check" size={10} />{:else}{i + 1}{/if}
            </span>
            <span class="desc">{step.description}</span>
          </li>
        {/each}
      </ol>
    {/if}
  </section>

  <section class="sec">
    <h3 class="sec-title"><Icon name="bolt" size={11} /> Tokens</h3>
    <div class="meter" role="img" aria-label="context usage">
      <div
        class="meter-fill"
        style:width="{Math.min(100, session.ctxRatio * 100)}%"
      ></div>
    </div>
    <div class="meter-nums mono">
      <span title="context window fill">ctx {fmt(session.ctxUsed)}/{fmtCtx(session.ctxLimit)}</span>
      <span>{fmt(session.usage.input)} in</span>
      <span class="dim">{fmt(session.usage.cached_input)} cached</span>
      <span>{fmt(session.usage.output)} out</span>
      <span class="dim">{fmt(session.usage.thinking)} think</span>
    </div>
    <div class="cost-row mono">
      <span>cache hit {Math.round(session.cacheRatio * 100)}%</span>
      <span>${session.costUsd.toFixed(4)}</span>
    </div>
    <div class="model-row mono">
      <span class="dim">{session.provider}</span>
      <span class="model-name">{session.model}</span>
    </div>
    <div class="sess-row mono" title="session stats">
      <span>{session.sessionTurns} turns</span>
      <span>{elapsed}</span>
      <span>{fmt(sessionTokTotal)} tok</span>
    </div>
  </section>

  <section class="sec">
    <h3 class="sec-title"><Icon name="clock" size={11} /> Checkpoints</h3>
    {#if session.checkpoints.length === 0}
      <p class="dim">None yet — snapshots are taken before writes.</p>
    {:else}
      <ul class="cps">
        {#each session.checkpoints as cp}
          <li class="cp">
            <span class="cp-id mono">{cp.id}</span>
            <span class="cp-label">{cp.label}</span>
            <span class="cp-at mono dim">{cp.at}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="sec">
    <h3 class="sec-title"><Icon name="diff" size={11} /> Changed files</h3>
    {#if session.changedFiles.length === 0}
      <p class="dim">No changes yet.</p>
    {:else}
      <ul class="files">
        {#each session.changedFiles as f}
          <li class="file">
            <span class="fpath mono" title={f.path}>{f.path}</span>
            <span class="fstat mono"><span class="add">+{f.added}</span> <span class="del">-{f.removed}</span></span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>

<style>
  .inspector {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px;
    overflow-y: auto;
    height: 100%;
  }
  .sec {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 10px;
    background: var(--bg-1);
  }
  .sec-title {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 8px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .dim {
    color: var(--fg-3);
    font-size: 11.5px;
    margin: 0;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .step {
    display: flex;
    gap: 8px;
    font-size: 12px;
    color: var(--fg-2);
    align-items: baseline;
  }
  .step.done {
    color: var(--fg-3);
  }
  .step.done .desc {
    text-decoration: line-through;
    text-decoration-color: var(--fg-3);
  }
  .step.active {
    color: var(--fg-0);
  }
  .marker {
    width: 16px;
    flex: none;
    text-align: center;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--fg-3);
  }
  .step.active .marker {
    color: var(--accent);
  }
  .meter {
    height: 5px;
    background: var(--bg-3);
    border-radius: 3px;
    overflow: hidden;
    margin-bottom: 6px;
  }
  .meter-fill {
    height: 100%;
    background: var(--accent);
    transition: width var(--motion);
  }
  .meter-nums {
    display: flex;
    gap: 10px;
    font-size: 10.5px;
    color: var(--fg-1);
    flex-wrap: wrap;
  }
  .cost-row,
  .model-row {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    font-size: 10.5px;
    color: var(--fg-1);
    margin-top: 6px;
  }
  .model-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sess-row {
    display: flex;
    gap: 12px;
    font-size: 10px;
    color: var(--fg-2);
    margin-top: 7px;
    flex-wrap: wrap;
  }
  .cps,
  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .cp {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 11.5px;
  }
  .cp-id {
    color: var(--accent);
    font-size: 10.5px;
    flex: none;
  }
  .cp-label {
    flex: 1;
    min-width: 0;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cp-at {
    font-size: 10px;
    flex: none;
  }
  .file {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 11.5px;
  }
  .fpath {
    flex: 1;
    min-width: 0;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fstat {
    flex: none;
    font-size: 10.5px;
  }
  .add {
    color: var(--ok);
  }
  .del {
    color: var(--err);
  }
</style>
