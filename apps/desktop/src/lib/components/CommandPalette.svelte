<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import { THEMES } from '../themes';
  import Icon from './Icon.svelte';
  import { trapFocus } from '../focus';

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  interface Command {
    name: string;
    hint: string;
    group: 'session' | 'model' | 'view' | 'export' | 'settings';
    keys?: string;
    run?: () => void;
    disabled?: string;
  }

  const GROUPS: { id: Command['group']; label: string }[] = [
    { id: 'session', label: 'Session' },
    { id: 'model', label: 'Model' },
    { id: 'view', label: 'View' },
    { id: 'export', label: 'Export' },
    { id: 'settings', label: 'Settings' },
  ];

  const commands = $derived<Command[]>([
    { name: '/new', hint: 'start a fresh chat', group: 'session', keys: '⌘N', run: () => session.startFresh() },
    { name: '/sidebar', hint: session.sidebarCollapsed ? 'show the sidebar' : 'hide the sidebar', group: 'view', keys: '⌘B', run: () => (session.sidebarCollapsed = !session.sidebarCollapsed) },
    { name: '/clear', hint: 'clear this transcript', group: 'session', run: () => session.clearTranscript() },
    { name: '/compact', hint: 'compact context now', group: 'session', run: () => void session.compact() },
    { name: '/retry', hint: 'send the last message again', group: 'session', run: () => session.retryLast() },
    { name: '/copy', hint: 'copy the last reply', group: 'session', run: () => void session.copyLastReply() },
    { name: '/plan', hint: session.planMode ? 'leave plan mode' : 'read-only investigation, then a plan you approve', group: 'session', run: () => session.togglePlan() },
    { name: '/status', hint: 'session token stats', group: 'session', run: () => session.note(statusText()) },
    { name: '/help', hint: 'show shortcuts', group: 'session', run: () => session.note(HELP_TEXT) },
    { name: '/model', hint: `cycle model (now ${session.model || '—'})`, group: 'model', run: () => void session.cycleModel() },
    { name: '/thinking', hint: `cycle thinking level (now ${session.thinkingLevel})`, group: 'model', run: () => session.cycleThinking() },
    { name: '/theme', hint: `cycle palette (now ${session.theme})`, group: 'view', run: () => cycleTheme() },
    { name: '/glass', hint: `experimental glass effect (now ${session.glass ? 'on' : 'off'})`, group: 'view', run: () => session.setGlass(!session.glass) },
    { name: '/density', hint: `transcript density (now ${session.density})`, group: 'view', run: () => session.setDensity(session.density === 'compact' ? 'comfortable' : 'compact') },
    { name: '/activity', hint: session.panelOpen ? 'hide the side panel' : 'show tokens, plan and checkpoints', group: 'view', run: () => (session.panelOpen ? session.setPanel(false) : session.openActivity()) },
    { name: '/export md', hint: 'write the transcript as Markdown', group: 'export', run: () => void session.exportTo('md') },
    { name: '/export html', hint: 'self-contained HTML, images inline', group: 'export', run: () => void session.exportTo('html') },
    { name: '/export json', hint: 'structured transcript for tooling', group: 'export', run: () => void session.exportTo('json') },
    { name: '/settings', hint: 'providers, MCP, usage', group: 'settings', keys: '⌘,', run: () => session.setMode('settings') },
    { name: '/providers', hint: 'add or switch a provider', group: 'settings', run: () => session.openSettings('providers') },
    { name: '/usage', hint: 'spend, cache savings, latency', group: 'settings', run: () => session.openSettings('usage') },
    { name: '/shortcuts', hint: 'keyboard reference', group: 'settings', keys: '?', run: () => session.note(HELP_TEXT) },
  ]);

  function statusText(): string {
    const t = session.usage;
    const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`);
    const pct = session.cacheRatio > 0 ? `${Math.round(session.cacheRatio * 100)}%` : '—';
    const cost = session.costUsd > 0 ? `$${session.costUsd.toFixed(4)}` : '—';
    return [
      `model ${session.model || '—'} · thinking ${session.thinkingLevel}`,
      `ctx ${k(session.ctxUsed)} tok · in ${k(t.input)} (cached ${k(t.cached_input)}) · out ${k(t.output)} · think ${k(t.thinking)}`,
      `cache hit ${pct} · cost ${cost} · ${session.sessionTurns} turns`,
    ].join('\n');
  }

  const HELP_TEXT = [
    'Shortcuts:',
    '  Enter        send message',
    '  Shift+Enter  newline',
    '  Esc          cancel running turn',
    '  Shift+Tab    cycle thinking level',
    '  Up           recall the last message (empty composer)',
    '  ⌘K / Ctrl+K  command palette',
    '  ⌘, / Ctrl+,  settings',
    '  ⌘N / Ctrl+N  new chat',
    '  ⌘B / Ctrl+B  toggle the sidebar',
    '  /            command palette',
    '  @            attach file reference',
    '',
    'Messages sent while a turn runs are queued and sent in order.',
    '/plan investigates read-only and proposes steps; nothing changes until you run the plan.',
  ].join('\n');


  let filter = $state('');
  let sel = $state(0);
  let filterEl = $state<HTMLInputElement | null>(null);

  onMount(() => filterEl?.focus());

  /** Subsequence match with a contiguous-run bonus; returns null when no match. */
  function score(text: string, q: string): number | null {
    if (!q) return 0;
    const hay = text.toLowerCase();
    let ti = 0;
    let total = 0;
    let run = 0;
    for (const ch of q) {
      const at = hay.indexOf(ch, ti);
      if (at < 0) return null;
      run = at === ti && ti > 0 ? run + 1 : 0;
      total += 1 + run * 2 - Math.min(at - ti, 8) * 0.15;
      ti = at + 1;
    }
    return total;
  }

  const modelCommands = $derived<Command[]>(
    session.models.slice(0, 80).map((m) => ({
      name: m.id,
      hint: `${m.provider}${m.context_tokens ? ` · ${Math.round(m.context_tokens / 1000)}k ctx` : ''}`,
      group: 'model' as const,
      run: () => {
        void session.setModel(m.id, m.provider);
        session.paletteOpen = false;
      },
    })),
  );

  const all = $derived([...commands, ...modelCommands]);

  const matched = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return { commands, models: [] as Command[] };
    const scored = all
      .map((c) => ({ c, s: score(`${c.name} ${c.hint}`, q) }))
      .filter((x): x is { c: Command; s: number } => x.s !== null)
      .sort((a, b) => b.s - a.s);
    return {
      commands: scored.filter((x) => x.c.group !== 'model').map((x) => x.c),
      models: scored
        .filter((x) => x.c.group === 'model')
        .map((x) => x.c)
        .slice(0, 8),
    };
  });

  const visible = $derived(
    filter.trim() ? [...matched.commands, ...matched.models] : commands,
  );

  $effect(() => {
    void filter;
    sel = 0;
  });

  function cycleTheme(): void {
    const ids = THEMES.map((t) => t.id);
    const i = ids.indexOf(session.theme);
    session.setTheme(ids[(i + 1) % ids.length] ?? 'auto');
  }

  function close(): void {
    session.paletteOpen = false;
    onclose();
  }

  function choose(c: Command): void {
    if (c.disabled || !c.run) return;
    c.run();
    close();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
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

  function groupLabel(id: Command['group']): string {
    return GROUPS.find((g) => g.id === id)?.label ?? id;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={(e) => {
    if (e.target === e.currentTarget) close();
  }}
  onkeydown={onKeydown}
  role="presentation"
>
  <div class="palette rise" role="dialog" aria-label="Command palette" tabindex="-1" use:trapFocus>
    <div class="filter-wrap">
      <Icon name="terminal" size={13} />
      <input
        class="filter"
        placeholder="Command, model, or action…"
        bind:value={filter}
        onkeydown={onKeydown}
        aria-label="Filter commands"
        bind:this={filterEl}
      />
      <kbd class="esc-key mono">esc</kbd>
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
            <span class="cgroup mono">
              {c.keys ?? groupLabel(c.group)}
            </span>
          </button>
        </li>
      {/each}
      {#if visible.length === 0}
        <li class="none">no matching command or model</li>
      {/if}
    </ul>
    <div class="foot mono">
      <span>↑↓ move</span>
      <span>⏎ run</span>
      <span>esc close</span>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.4);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 14vh;
    z-index: 50;
  }
  .palette {
    width: 520px;
    max-width: 92vw;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    overflow: hidden;
  }
  .filter-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 11px 13px;
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
  .esc-key {
    font-size: 9.5px;
    color: var(--fg-3);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 1px 4px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 5px;
    max-height: 320px;
    overflow: auto;
  }
  .cmd {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 30px;
    padding: 0 9px;
    border-radius: 8px;
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
    width: 118px;
    flex: none;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chint {
    font-size: 11.5px;
    color: var(--fg-2);
    flex: 1 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cgroup {
    flex: none;
    font-size: 9.5px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .none {
    padding: 14px;
    text-align: center;
    color: var(--fg-3);
    font-size: 12px;
  }
  .foot {
    display: flex;
    gap: 14px;
    padding: 7px 13px;
    border-top: 1px solid var(--border);
    font-size: 10px;
    color: var(--fg-3);
  }
</style>
