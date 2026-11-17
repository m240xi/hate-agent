<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import ProviderLogo from './ProviderLogo.svelte';

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
</script>

<div class="inspector">
  <section class="sec">
    <h3 class="section-title"><Icon name="list-ordered" size={11} /> Plan</h3>
    {#if session.plan.length === 0}
      <div class="empty">
        <p class="empty-title">No plan yet</p>
        <p class="empty-body">
          Turn on plan mode in the composer and the agent investigates read-only,
          then proposes steps for you to approve.
        </p>
      </div>
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
    <h3 class="section-title"><Icon name="coins" size={11} /> Tokens</h3>
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
      <span title="session cache hit ratio — cached input / total input">cache hit {Math.round(session.cacheRatio * 100)}%</span>
      <span>${session.costUsd.toFixed(4)}</span>
    </div>
    <div class="model-row mono">
      <span class="prov-chip dim">
        {#if session.providerName || session.provider}
          <ProviderLogo provider={session.providerName || session.provider} size={11} fallbackFontSize={12} />
        {/if}
        <span>{session.provider}</span>
      </span>
      <span class="model-name">{session.model}</span>
    </div>
    <div class="sess-row mono" title="session stats">
      <span>{session.sessionTurns} turns</span>
      <span>{elapsed}</span>
      <span>{fmt(sessionTokTotal)} tok</span>
    </div>
  </section>

  {#if session.workspacePath && session.worktree?.is_git}
  <section class="sec">
    <h3 class="section-title">
      <Icon name="branch" size={11} /> Git Worktree
      <span class="spacer"></span>
      <span class="wt-branch mono">{session.worktree.branch}</span>
    </h3>
    {#if session.worktree.is_worktree}
      <p class="empty-body" style="margin-bottom: 8px">
        Isolated branch active{session.worktree.dirty ? ' (uncommitted changes)' : ''}.
      </p>
      <div class="wt-actions">
        <button
          class="cp-rewind"
          onclick={() => void session.mergeBackWorktree()}
          disabled={session.running}
          title="Commit any changes and merge back into the main branch"
        >
          Merge back
        </button>
        <button
          class="cp-rewind danger"
          onclick={() => void session.discardWorktree(session.worktree?.dirty ?? false)}
          disabled={session.running}
          title="Remove this worktree and return to the main repository"
        >
          {session.worktree.dirty ? 'Force discard' : 'Return to main'}
        </button>
      </div>
    {:else}
      <p class="empty-body" style="margin-bottom: 8px">
        Spin up an isolated git worktree so edits don't touch your main working tree until merged.
      </p>
      <button
        class="cp-rewind"
        onclick={() => void session.enterWorktree()}
        disabled={session.running}
      >
        Isolate in worktree
      </button>
    {/if}
  </section>
  {/if}

  {#if session.workspacePath}
  <section class="sec">
    <h3 class="section-title">
      <Icon name="book" size={11} /> Workspace Rules
      <span class="spacer"></span>
      <button class="sec-btn" onclick={() => void session.createRulesFile('.hate-agent.md')} title="Create or open .hate-agent.md">
        <Icon name="plus" size={10} />
      </button>
    </h3>
    {#if session.workspaceRules.length === 0}
      <div class="empty">
        <p class="empty-title">No instruction files</p>
        <p class="empty-body">
          Add <code>.hate-agent.md</code> or <code>AGENTS.md</code> to set persistent project rules.
        </p>
      </div>
    {:else}
      <ul class="files">
        {#each session.workspaceRules as r (r.file)}
          <li class="file">
            <button class="fpath mono" onclick={() => void session.openFile(r.file)} title={r.preview}>
              <span class="rule-dot" class:on={r.active}>{r.active ? '●' : '○'}</span>
              {r.file}
            </button>
            <span class="fstat mono">{r.bytes} B</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
  {/if}

  {#if session.workspacePath || session.checkpoints.length > 0}
  <section class="sec">
    <h3 class="section-title"><Icon name="history" size={11} /> Checkpoints</h3>
    {#if session.checkpoints.length === 0}
      <div class="empty">
        <p class="empty-title">No checkpoints yet</p>
        <p class="empty-body">
          A snapshot of the touched files is taken before the first write, so you can
          roll the folder back to it.
        </p>
      </div>
    {:else}
      <ul class="cps">
        {#each session.checkpoints as cp}
          <li class="cp">
            <span class="cp-id mono" title={cp.id}>{cp.id.slice(0, 8)}</span>
            <span class="cp-label">{cp.label}</span>
            <span class="cp-at mono dim">{cp.at}</span>
            <button
              class="cp-rewind"
              title="Restore this folder's files to this checkpoint"
              onclick={() => session.rewindCheckpoint(cp.id)}
            >
              rewind
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
  {/if}

  {#if session.workspacePath}
  <section class="sec">
    <h3 class="section-title">
      <Icon name="diff" size={11} /> Changed files
      <span class="spacer"></span>
      <button class="sec-btn" onclick={() => void session.refreshChanges()} title="Refresh from git">
        <Icon name="refresh" size={10} />
      </button>
    </h3>
    {#if session.changedFiles.length === 0}
      <div class="empty">
        <p class="empty-title">No changes yet</p>
        <p class="empty-body">
          Files the agent edits show up here with a one-click diff.
        </p>
      </div>
    {:else}
      <ul class="files">
        {#each session.changedFiles as f}
          <li class="file">
            <button class="fpath mono" title={`${f.path} — click to review the diff`} onclick={() => void session.openDiff(f.path)}>
              {#if f.status}<span class="fstatus" data-s={f.status}>{f.status}</span>{/if}
              {f.path}
            </button>
            <span class="fstat mono"><span class="add">+{f.added}</span> <span class="del">-{f.removed}</span></span>
            <button
              class="sec-btn revert-btn"
              onclick={() => void session.revertChangedFile(f.path)}
              title={`Revert ${f.path}`}
              aria-label={`Revert ${f.path}`}
            >
              <Icon name="undo" size={10} />
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
  {/if}
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
    flex: none;
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 10px;
    background: var(--bg-1);
  }
  .spacer {
    flex: 1;
  }
  .sec-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: var(--radius);
    color: var(--fg-3);
  }
  .sec-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .revert-btn:hover {
    color: var(--err);
  }
  .wt-branch {
    font-size: 10px;
    color: var(--accent);
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wt-actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .rule-dot {
    color: var(--fg-3);
    margin-right: 4px;
    font-size: 9px;
  }
  .rule-dot.on {
    color: var(--ok);
  }
  .empty {
    padding: 8px 10px;
    border: 1px dashed var(--border);
    border-radius: 10px;
  }
  .empty-title {
    margin: 0 0 4px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--fg-2);
  }
  .empty-body {
    margin: 0;
    font-size: 10.5px;
    line-height: 1.55;
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
  .desc {
    min-width: 0;
    overflow-wrap: anywhere;
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
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 10.5px;
    color: var(--fg-1);
    margin-top: 6px;
  }
  .prov-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
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
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cp-rewind {
    flex: none;
    font-size: 10px;
    padding: 2px 7px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-2);
    color: var(--fg-2);
  }
  .cp-rewind:hover:not(:disabled) {
    color: var(--warn);
    border-color: var(--warn);
  }
  .cp-rewind.danger:hover:not(:disabled) {
    color: var(--err);
    border-color: var(--err);
  }
  .cp-rewind:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .file {
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 11.5px;
  }
  .fpath {
    flex: 1;
    min-width: 0;
    text-align: left;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fpath:hover {
    color: var(--accent);
  }
  .fstatus {
    display: inline-block;
    width: 12px;
    margin-right: 4px;
    color: var(--fg-3);
    font-size: 10px;
  }
  .fstatus[data-s='M'] {
    color: var(--warn);
  }
  .fstatus[data-s='A'] {
    color: var(--ok);
  }
  .fstatus[data-s='D'] {
    color: var(--err);
  }
  .fstat {
    flex: none;
    font-size: 10px;
    color: var(--fg-3);
  }
  .add {
    color: var(--ok);
  }
  .del {
    color: var(--err);
  }
</style>
