<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import { THEMES } from '../themes';
  import Icon from './Icon.svelte';

  // `/` command palette. Only commands that do real work today are
  // enabled; backend-dependent ones render disabled with a reason.
  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  interface Command {
    name: string;
    hint: string;
    run?: () => void;
    disabled?: string;
  }

  const commands = $derived<Command[]>([
    {
      name: '/clear',
      hint: 'clear transcript',
      run: () => session.clearTranscript(),
    },
    {
      name: '/thinking',
      hint: 'cycle thinking level',
      run: () => session.cycleThinking(),
    },
    {
      name: '/theme',
      hint: 'cycle theme',
      run: () => cycleTheme(),
    },
    {
      name: '/help',
      hint: 'show shortcuts',
      run: () => session.note(HELP_TEXT),
    },
    {
      name: '/model',
      hint: 'cycle model',
      run: () => void session.cycleModel(),
    },
    {
      name: '/compact',
      hint: 'compact context now',
      run: () => void session.compact(),
    },
    {
      name: '/status',
      hint: 'session token stats',
      run: () => session.note(statusText()),
    },
    {
      name: '/plan',
      hint: session.planMode
        ? 'leave plan mode'
        : 'plan mode: read-only investigation, then a plan you approve',
      run: () => session.togglePlan(),
    },
    {
      name: '/new',
      hint: 'start a new session',
      run: () => void session.newSession(),
    },
    {
      name: '/retry',
      hint: 'send the last message again',
      run: () => session.retryLast(),
    },
    {
      name: '/copy',
      hint: 'copy the last reply',
      run: () => void session.copyLastReply(),
    },
    {
      name: '/export',
      hint: 'copy the transcript as Markdown',
      run: () => void session.exportTranscript(),
    },
  ]);

  function statusText(): string {
    const t = session.sessionTotals;
    const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`);
    const pct = session.cacheRatio > 0 ? `${Math.round(session.cacheRatio * 100)}%` : '—';
    const cost = session.costUsd > 0 ? `$${session.costUsd.toFixed(4)}` : '—';
    return [
      `model ${session.model || '—'} · thinking ${session.thinkingLevel}`,
      `ctx ${k(session.ctxUsed)} tok · in ${k(t.input)} (cached ${k(t.cached_input)}) · out ${k(t.output)} · think ${k(t.thinking)}`,
      `cache hit ${pct} · cost ${cost}`,
    ].join('\n');
  }

  const HELP_TEXT = [
    'Shortcuts:',
    '  Enter        send message',
    '  Shift+Enter  newline',
    '  Esc          cancel running turn',
    '  Shift+Tab    cycle thinking level',
    '  Up           recall the last message (empty composer)',
    '  /            command palette',
    '  @            attach file reference',
    '',
    'Messages sent while a turn runs are queued and sent in order.',
    '/plan investigates read-only and proposes steps; nothing changes until you run the plan.',
    '',
    'Every token is attributable: each reply shows its request usage, your message shows',
    'the whole turn, and tool cards show full → sent counts.',
  ].join('\n');

  let filter = $state('');
  let sel = $state(0);
  let filterEl = $state<HTMLInputElement | null>(null);

  onMount(() => filterEl?.focus());

  const visible = $derived(
    commands.filter((c) => c.name.includes(filter.trim().toLowerCase())),
  );

  function cycleTheme(): void {
    const ids = THEMES.map((t) => t.id);
    const i = ids.indexOf(session.theme);
    session.setTheme(ids[(i + 1) % ids.length] ?? 'auto');
  }

  function choose(c: Command): void {
    if (c.disabled || !c.run) return;
    c.run();
    onclose();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      sel = Math.min(sel + 1, visible.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      sel = Math.max(sel - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const c = visible[sel];
      if (c) choose(c);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={(e) => {
    if (e.target === e.currentTarget) onclose();
  }}
  onkeydown={onKeydown}
  role="presentation"
>
  <div class="palette rise" role="dialog" aria-label="Command palette" tabindex="-1">
    <div class="filter-wrap">
      <Icon name="terminal" size={13} />
      <input
        class="filter"
        placeholder="Type a command…"
        bind:value={filter}
        onkeydown={onKeydown}
        aria-label="Filter commands"
        bind:this={filterEl}
      />
    </div>
    <ul class="list" role="listbox">
      {#each visible as c, i}
        <li>
          <button
            class="cmd"
            class:sel={i === sel}
            class:disabled={!!c.disabled}
            onclick={() => choose(c)}
            onmouseenter={() => (sel = i)}
            role="option"
            aria-selected={i === sel}
            disabled={!!c.disabled}
          >
            <span class="cname mono">{c.name}</span>
            <span class="chint">{c.disabled ?? c.hint}</span>
          </button>
        </li>
      {/each}
      {#if visible.length === 0}
        <li class="none">no matching commands</li>
      {/if}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 15vh;
    z-index: 50;
  }
  .palette {
    width: 420px;
    max-width: 90vw;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 8px 30px rgb(0 0 0 / 0.4);
    overflow: hidden;
  }
  .filter-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-2);
  }
  .filter {
    flex: 1;
    background: none;
    border: none;
    outline: none;
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--fg-0);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 280px;
    overflow: auto;
  }
  .cmd {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 30px;
    padding: 0 10px;
    border-radius: var(--radius);
    text-align: left;
    color: var(--fg-1);
  }
  .cmd.sel {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .cmd.disabled {
    opacity: 0.45;
    cursor: default;
  }
  .cname {
    font-size: 12px;
    color: var(--accent);
    width: 90px;
    flex: none;
  }
  .chint {
    font-size: 11.5px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .none {
    padding: 12px;
    text-align: center;
    color: var(--fg-3);
    font-size: 12px;
  }
</style>
