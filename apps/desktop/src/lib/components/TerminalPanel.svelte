<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import '@xterm/xterm/css/xterm.css';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  interface Props {
    dock: 'bottom' | 'right';
  }

  let { dock }: Props = $props();

  interface MountedTerm {
    term: Terminal;
    fit: FitAddon;
    host: HTMLDivElement;
    replayed: boolean;
  }

  const mounted = new Map<string, MountedTerm>();
  let stageEl = $state<HTMLDivElement | null>(null);
  let shellMenuOpen = $state(false);
  let dragging = $state(false);

  const activeId = $derived(session.activeTerminalId ?? session.terminals[0]?.id ?? null);

  function readTheme(): Record<string, string> {
    if (typeof document === 'undefined') return {};
    const cs = getComputedStyle(document.documentElement);
    const v = (name: string, fallback: string) => cs.getPropertyValue(name).trim() || fallback;
    const bg = v('--bg-0', '#111216');
    const fg = v('--fg-0', '#eceef2');
    const accent = v('--accent', '#7aa2f7');
    const sel = v('--bg-3', '#2a2f3d');
    const ok = v('--ok', '#73daca');
    const warn = v('--warn', '#e0af68');
    const err = v('--err', '#f7768e');
    return {
      background: bg,
      foreground: fg,
      cursor: accent,
      cursorAccent: bg,
      selectionBackground: sel,
      red: err,
      green: ok,
      yellow: warn,
      blue: accent,
    };
  }

  function syncThemeAll(): void {
    const theme = readTheme();
    for (const m of mounted.values()) {
      m.term.options.theme = theme;
    }
  }

  function fitActive(): void {
    if (!activeId) return;
    const m = mounted.get(activeId);
    if (!m || !m.host.offsetParent) return;
    try {
      m.fit.fit();
      void session.resizeTerminal(activeId, m.term.cols, m.term.rows);
    } catch {
      // ignore transient zero-size layouts
    }
  }

  function attachTerm(node: HTMLDivElement, id: string) {
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 12,
      lineHeight: 1.35,
      fontFamily: "'JetBrains Mono', 'Cascadia Code', ui-monospace, SFMono-Regular, Menlo, monospace",
      theme: readTheme(),
      scrollback: 5000,
      allowProposedApi: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(node);

    const entry: MountedTerm = { term, fit, host: node, replayed: false };
    mounted.set(id, entry);

    const onDataSub = term.onData((data) => {
      void session.writeTerminal(id, data);
    });

    void session.readTerminalBuffer(id, true).then((raw) => {
      if (raw && mounted.get(id) === entry) {
        term.write(raw);
      }
      entry.replayed = true;
      queueMicrotask(() => {
        if (id === activeId) {
          fitActive();
          term.focus();
        }
      });
    });

    return {
      destroy() {
        onDataSub.dispose();
        term.dispose();
        if (mounted.get(id) === entry) mounted.delete(id);
      },
    };
  }

  onMount(() => {
    const offOut = session.onTerminalOutput((id, data) => {
      const m = mounted.get(id);
      if (m) m.term.write(data);
    });

    let ro: ResizeObserver | null = null;
    if (stageEl) {
      ro = new ResizeObserver(() => fitActive());
      ro.observe(stageEl);
    }

    return () => {
      offOut();
      ro?.disconnect();
    };
  });

  $effect(() => {
    void session.theme;
    void session.glass;
    syncThemeAll();
  });

  $effect(() => {
    const id = activeId;
    if (!id) return;
    void tick().then(() => {
      fitActive();
      mounted.get(id)?.term.focus();
    });
  });

  function clearActive(): void {
    if (!activeId) return;
    mounted.get(activeId)?.term.clear();
  }

  function startResize(e: PointerEvent): void {
    if (dock !== 'bottom') return;
    e.preventDefault();
    dragging = true;
    const startY = e.clientY;
    const startH = session.terminalHeight;
    const move = (ev: PointerEvent) => {
      session.setTerminalHeight(startH + (startY - ev.clientY));
      fitActive();
    };
    const up = () => {
      dragging = false;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }
</script>

<section
  class="term-panel"
  class:bottom={dock === 'bottom'}
  class:right={dock === 'right'}
  style:height={dock === 'bottom' ? `${session.terminalHeight}px` : undefined}
  aria-label="Workspace terminal"
>
  {#if dock === 'bottom'}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="term-resizer"
      class:active={dragging}
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize terminal"
      onpointerdown={startResize}
    ></div>
  {/if}

  <header class="term-head">
    <div class="term-tabs" role="tablist" aria-label="Terminal tabs">
      {#each session.terminals as t (t.id)}
        <div
          class="t-tab"
          class:on={t.id === activeId}
          role="tab"
          aria-selected={t.id === activeId}
        >
          <button
            class="t-tab-btn mono"
            onclick={() => (session.activeTerminalId = t.id)}
            title={`${t.shell} — ${t.cwd}`}
          >
            <Icon name="terminal" size={11} />
            <span class="t-title">{t.title}</span>
            {#if !t.alive}
              <span class="t-dead" title={`Exited (${t.exit_code ?? 0})`}>·</span>
            {/if}
          </button>
          <button
            class="t-close"
            onclick={() => void session.closeTerminal(t.id)}
            aria-label={`Close ${t.title}`}
            title="Close tab"
          >
            <Icon name="x" size={10} />
          </button>
        </div>
      {/each}

      <div class="t-new-wrap">
        <button
          class="t-icon-btn"
          onclick={() => void session.createTerminal()}
          title="New terminal tab"
          aria-label="New terminal tab"
        >
          <Icon name="plus" size={12} />
        </button>
        {#if session.terminalShells.length > 1}
          <button
            class="t-icon-btn sm"
            onclick={() => (shellMenuOpen = !shellMenuOpen)}
            title="Choose shell"
            aria-label="Choose shell"
          >
            <Icon name="chevron-down" size={11} />
          </button>
          {#if shellMenuOpen}
            <div class="shell-pop pop-panel" role="menu">
              {#each session.terminalShells as sh (sh.id)}
                <button
                  class="shell-item mono"
                  role="menuitem"
                  onclick={() => {
                    shellMenuOpen = false;
                    void session.createTerminal(sh.id);
                  }}
                >
                  {sh.label}
                </button>
              {/each}
            </div>
          {/if}
        {/if}
      </div>
    </div>

    <span class="spacer"></span>

    <div class="term-actions">
      {#if session.workspacePath && activeId}
        <button
          class="t-act mono"
          onclick={() => void session.cdTerminalToWorkspace()}
          title="Change directory to attached workspace"
        >
          cd
        </button>
      {/if}
      {#if activeId}
        <button
          class="t-act mono"
          onclick={() => session.attachTerminalToChat()}
          title="Attach recent terminal output to the next message"
        >
          <Icon name="paperclip" size={11} />
          <span class="lbl">Attach</span>
        </button>
        <button
          class="t-act mono accent"
          onclick={() => void session.askAiToFixTerminal()}
          disabled={session.running}
          title="Ask AI to inspect this terminal output and fix errors"
        >
          <Icon name="sparkles" size={11} />
          <span class="lbl">Ask AI</span>
        </button>
        <button class="t-icon-btn" onclick={clearActive} title="Clear terminal" aria-label="Clear terminal">
          <Icon name="trash" size={11} />
        </button>
      {/if}
      <button
        class="t-icon-btn"
        onclick={() => session.setTerminalDock(dock === 'bottom' ? 'right' : 'bottom')}
        title={dock === 'bottom' ? 'Dock terminal to right side panel' : 'Dock terminal to bottom drawer'}
        aria-label="Toggle terminal dock position"
      >
        <Icon name={dock === 'bottom' ? 'panel-right' : 'columns'} size={12} />
      </button>
      {#if dock === 'bottom'}
        <button
          class="t-icon-btn"
          onclick={() => (session.terminalOpen = false)}
          title="Hide terminal (Ctrl+`)"
          aria-label="Hide terminal"
        >
          <Icon name="x" size={12} />
        </button>
      {/if}
    </div>
  </header>

  <div class="term-stage" bind:this={stageEl}>
    {#if session.terminals.length === 0}
      <div class="term-empty">
        <p class="dim">No terminal open.</p>
        <button class="btn sm" onclick={() => void session.createTerminal()}>
          <Icon name="terminal" size={12} /> Open terminal
        </button>
      </div>
    {:else}
      {#each session.terminals as t (t.id)}
        <div
          class="term-slot"
          class:active={t.id === activeId}
          use:attachTerm={t.id}
        ></div>
      {/each}
    {/if}
  </div>
</section>

<style>
  .term-panel {
    position: relative;
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    background: var(--bg-0);
  }
  .term-panel.bottom {
    flex: none;
    border-top: 1px solid var(--border);
  }
  .term-panel.right {
    flex: 1;
    height: 100%;
  }
  .term-resizer {
    position: absolute;
    top: -2px;
    left: 0;
    right: 0;
    height: 5px;
    cursor: row-resize;
    z-index: 6;
  }
  .term-resizer:hover,
  .term-resizer.active {
    background: var(--accent-dim);
    opacity: 0.6;
  }
  .term-head {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 34px;
    padding: 0 8px;
    background: var(--bg-1);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .term-tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    overflow-x: auto;
  }
  .t-tab {
    display: inline-flex;
    align-items: center;
    height: 24px;
    border-radius: 6px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--fg-2);
    flex: none;
  }
  .t-tab.on {
    background: var(--bg-0);
    border-color: var(--border);
    color: var(--fg-0);
  }
  .t-tab-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 100%;
    padding: 0 4px 0 8px;
    font-size: 11px;
  }
  .t-title {
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .t-dead {
    color: var(--warn);
    font-weight: 700;
  }
  .t-close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    margin-right: 2px;
    border-radius: 4px;
    color: var(--fg-3);
  }
  .t-close:hover {
    color: var(--err);
    background: var(--bg-2);
  }
  .t-new-wrap {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 1px;
  }
  .shell-pop {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    min-width: 160px;
    padding: 4px;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--el-3);
    z-index: 40;
    display: flex;
    flex-direction: column;
  }
  .shell-item {
    text-align: left;
    padding: 5px 8px;
    border-radius: 5px;
    font-size: 11px;
    color: var(--fg-1);
  }
  .shell-item:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .term-actions {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }
  .t-act {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    padding: 0 7px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-0);
    color: var(--fg-2);
    font-size: 10.5px;
  }
  .t-act:hover:not(:disabled) {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .t-act.accent {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .t-act:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .t-icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    color: var(--fg-2);
  }
  .t-icon-btn.sm {
    width: 16px;
  }
  .t-icon-btn:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .term-panel.right .lbl {
    display: none;
  }
  .spacer {
    flex: 1;
  }
  .term-stage {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    padding: 4px 6px;
  }
  .term-slot {
    display: none;
    width: 100%;
    height: 100%;
  }
  .term-slot.active {
    display: block;
  }
  .term-slot :global(.xterm) {
    height: 100%;
  }
  .term-empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
  }
  .dim {
    margin: 0;
    font-size: 12px;
    color: var(--fg-3);
  }
</style>
