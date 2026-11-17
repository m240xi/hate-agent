<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { fade } from 'svelte/transition';
  import { session } from './lib/store.svelte';
  import Landing from './lib/components/Landing.svelte';
  import ContextMenu from './lib/components/ContextMenu.svelte';
  import SettingsView from './lib/components/SettingsView.svelte';
  import Notice from './lib/components/Notice.svelte';
  import CommandPalette from './lib/components/CommandPalette.svelte';
  import QuickOpen from './lib/components/QuickOpen.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import SidePanel from './lib/components/SidePanel.svelte';
  import Conversation from './lib/components/Conversation.svelte';
  import Icon from './lib/components/Icon.svelte';
  import { renderMarkdown } from './lib/markdown';
  import { trapFocus } from './lib/focus';
  import { dragRegion, minimize, toggleMaximize, close } from './lib/window';

  let inTauri = $state(false);

  onMount(() => {
    inTauri = '__TAURI_INTERNALS__' in window;
    void session.start();
    let unlistenDrop: (() => void) | null = null;
    let disposed = false;
    if (inTauri) {
      void import('@tauri-apps/api/webview').then(async ({ getCurrentWebview }) => {
        const off = await getCurrentWebview().onDragDropEvent((e) => {
          const p = e.payload;
          if (p.type === 'enter') session.dropping = session.mode === 'chat';
          else if (p.type === 'leave') session.dropping = false;
          else if (p.type === 'drop') void session.handleDrop(p.paths);
        });
        if (disposed) off();
        else unlistenDrop = off;
      });
    }
    return () => {
      disposed = true;
      unlistenDrop?.();
      session.destroy();
    };
  });

  function onWindowKeydown(e: KeyboardEvent): void {
    const mod = e.ctrlKey || e.metaKey;
    const el = e.target instanceof Element ? e.target : null;
    const typing = !!el?.closest('input, textarea, select, [contenteditable="true"]');

    if (mod && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      session.paletteOpen = !session.paletteOpen;
      return;
    }
    if (mod && (e.code === 'Backquote' || e.key === '`') && session.mode === 'chat') {
      e.preventDefault();
      session.toggleTerminal();
      return;
    }
    if (mod && e.key.toLowerCase() === 'p' && !e.shiftKey && session.mode === 'chat') {
      e.preventDefault();
      session.quickOpen = !session.quickOpen;
      return;
    }
    if (mod && e.key.toLowerCase() === 'f' && !e.shiftKey && session.mode === 'chat') {
      if (!el?.closest('.xterm')) {
        e.preventDefault();
        session.transcriptSearchOpen = true;
      }
      return;
    }
    if (mod && e.key === ',') {
      e.preventDefault();
      session.setMode('settings');
      return;
    }
    if (mod && e.key.toLowerCase() === 'n' && !e.shiftKey) {
      e.preventDefault();
      session.startFresh();
      return;
    }
    if (mod && e.key.toLowerCase() === 'b' && !e.shiftKey) {
      e.preventDefault();
      session.sidebarCollapsed = !session.sidebarCollapsed;
      return;
    }
    if (e.key === '?' && !typing && !mod) {
      e.preventDefault();
      session.setMode('settings');
      return;
    }
    if (e.key !== 'Escape' || !session.viewer) return;
    if (typing) return;
    e.preventDefault();
    session.closeViewer();
  }

  $effect(() => {
    document.documentElement.dataset.density = session.density;
  });

  // Glass mode: a cursor-following rim light on each pane's edge. Positions are
  // written in the pane's own coordinates, so the gradient needs no
  // fixed-attachment background. One rAF loop eases toward the pointer and
  // stops once the light catches up. This follows input rather than playing
  // on its own, so it stays on even when the OS asks for reduced motion.
  let ambient = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (!session.glass) return;
    const rim = session.motion !== 'reduced';
    const parallax = session.ambientMotion === 'on';
    if (!rim && !parallax) return;
    let frame = 0;
    let targetX = 0;
    let targetY = 0;
    let x = 0;
    let y = 0;
    let primed = false;
    const paint = (): void => {
      frame = 0;
      x += (targetX - x) * 0.24;
      y += (targetY - y) * 0.24;
      const settled = Math.abs(targetX - x) < 0.5 && Math.abs(targetY - y) < 0.5;
      if (settled) {
        x = targetX;
        y = targetY;
      }
      if (rim) {
        for (const el of document.querySelectorAll<HTMLElement>('.glass-pane')) {
          const r = el.getBoundingClientRect();
          el.style.setProperty('--gx', `${(x - r.left).toFixed(1)}px`);
          el.style.setProperty('--gy', `${(y - r.top).toFixed(1)}px`);
        }
      }
      if (parallax) {
        ambient?.style.setProperty('--px', (x / window.innerWidth - 0.5).toFixed(3));
        ambient?.style.setProperty('--py', (y / window.innerHeight - 0.5).toFixed(3));
      }
      if (!settled) frame = requestAnimationFrame(paint);
    };
    const onMove = (e: PointerEvent): void => {
      targetX = e.clientX;
      targetY = e.clientY;
      if (!primed) {
        primed = true;
        x = targetX;
        y = targetY;
      }
      if (!frame) frame = requestAnimationFrame(paint);
    };
    window.addEventListener('pointermove', onMove, { passive: true });
    return () => {
      window.removeEventListener('pointermove', onMove);
      if (frame) cancelAnimationFrame(frame);
    };
  });

  {
    const r = Number(localStorage.getItem('hate:pane:right'));
    if (r) session.rightWidth = r;
    session.rightCollapsed = localStorage.getItem('hate:pane:rightCollapsed') === 'true';
    session.sidebarCollapsed = localStorage.getItem('hate:sidebar:collapsed') === 'true';
    session.chatPanelOpen = localStorage.getItem('hate:panel:chat') === 'true';
  }

  $effect(() => {
    localStorage.setItem('hate:pane:right', String(session.rightWidth));
    localStorage.setItem('hate:pane:rightCollapsed', String(session.rightCollapsed));
    localStorage.setItem('hate:sidebar:collapsed', String(session.sidebarCollapsed));
    localStorage.setItem('hate:panel:chat', String(session.chatPanelOpen));
  });

  const title = $derived(
    session.mode === 'settings'
      ? 'Settings'
      : session.sessionMeta?.title || 'New chat',
  );

  $effect(() => {
    const m = session.mode;
    untrack(() => {
      if (m === 'chat') void session.enterChat();
      else session.closeViewer();
    });
  });
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#snippet winControls()}
  {#if inTauri}
    <div class="win-controls">
      <button class="win-btn" onclick={minimize} aria-label="Minimize" title="Minimize">
        <Icon name="min" size={11} />
      </button>
      <button class="win-btn" onclick={toggleMaximize} aria-label="Maximize" title="Maximize">
        <Icon name="max" size={11} />
      </button>
      <button class="win-btn close" onclick={close} aria-label="Close" title="Close">
        <Icon name="x" size={12} />
      </button>
    </div>
  {/if}
{/snippet}

{#snippet updateBar()}
  {#if session.updateAvailable}
    <div class="updatebar" role="status">
      <span class="mono">
        update available — hate-agent v{session.updateAvailable.version}
        {#if session.updateAvailable.notes}
          <span class="update-notes" title={session.updateAvailable.notes}>?</span>
        {/if}
      </span>
      <span class="spacer"></span>
      {#if session.updateState === 'downloading'}
        <span class="mono dim">
          downloading…
          {#if session.updateTotal}
            {Math.round((session.updateDownloaded / session.updateTotal) * 100)}%
          {:else}
            {(session.updateDownloaded / 1048576).toFixed(1)} MB
          {/if}
        </span>
      {:else if session.updateState === 'installing'}
        <span class="mono dim">installing — the app will restart…</span>
      {:else}
        <button class="update-btn mono" onclick={() => session.installUpdate()}>
          Update &amp; restart
        </button>
        <button
          class="update-x"
          onclick={() => (session.updateAvailable = null)}
          aria-label="Dismiss">✕</button>
      {/if}
    </div>
  {/if}
{/snippet}

{#if session.glass}
  <div class="glass-ambient" bind:this={ambient} aria-hidden="true">
    <span class="orb a"></span>
    <span class="orb b"></span>
    <span class="orb c"></span>
    <span class="orb d"></span>
  </div>
{/if}

<div class="shell">
  {#if session.mode === 'home'}
    <header class="topbar" role="presentation" onmousedown={dragRegion}>
      <span class="spacer"></span>
      {@render winControls()}
    </header>
    {@render updateBar()}
    <div class="body" in:fade={{ duration: 140 }}>
      <Landing />
    </div>
  {:else}
    <div class="frame" role="presentation" onmousedown={(e) => e.target === e.currentTarget && dragRegion(e)}>
      {#if !session.sidebarCollapsed}
        <Sidebar />
      {/if}
      <div class="main">
        <header class="bar" role="presentation" onmousedown={dragRegion}>
          {#if session.sidebarCollapsed}
            <button
              class="bar-btn"
              onclick={() => (session.sidebarCollapsed = false)}
              aria-label="Open sidebar"
              title="Open sidebar (Ctrl+B)"
            >
              <Icon name="panel-left" size={16} />
            </button>
            <button
              class="bar-btn"
              onclick={() => session.startFresh()}
              aria-label="New chat"
              title="New chat (Ctrl+N)"
            >
              <Icon name="square-pen" size={16} />
            </button>
          {/if}
          <span class="title" title={title}>{title}</span>
          <span class="spacer"></span>
          {#if session.mode === 'chat'}
            <button
              class="bar-btn"
              class:on={(session.terminalOpen && session.terminalDock === 'bottom') || (session.panelOpen && session.panelTab === 'terminal')}
              onclick={() => session.toggleTerminal()}
              aria-label="Toggle workspace terminal"
              title="Toggle terminal (Ctrl+`)"
            >
              <Icon name="terminal" size={16} />
            </button>
            <button
              class="bar-btn"
              class:on={session.panelOpen}
              onclick={() => session.setPanel(!session.panelOpen)}
              aria-label={session.workspacePath ? 'Toggle files & activity' : 'Toggle activity'}
              title={session.workspacePath ? 'Files & activity' : 'Activity'}
            >
              <Icon name="panel-right" size={16} />
            </button>
          {/if}
          {@render winControls()}
        </header>
        {@render updateBar()}
        <div class="content">
          {#key session.mode}
            <div class="mode-view" in:fade={{ duration: 120 }}>
              {#if session.mode === 'settings'}
                <SettingsView />
              {:else}
                <Conversation />
                {#if session.panelOpen}
                  <SidePanel files={!!session.workspacePath} />
                {/if}
              {/if}
            </div>
          {/key}
        </div>
      </div>
    </div>
  {/if}

  {#if session.whatsNew}
    <div
      class="wn-scrim"
      role="presentation"
      onclick={() => session.dismissWhatsNew()}
      onkeydown={(e) => {
        if (e.key === 'Escape') session.dismissWhatsNew();
      }}
    >
      <div
        class="wn-card"
        role="dialog"
        aria-modal="true"
        aria-label="What's new"
        tabindex="-1"
        use:trapFocus
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => {
          if (e.key === 'Escape') session.dismissWhatsNew();
        }}
      >
        <div class="wn-head">
          <span class="wn-title mono">what's new in v{session.whatsNew.version}</span>
          <button
            class="wn-x"
            onclick={() => session.dismissWhatsNew()}
            aria-label="Close">✕</button>
        </div>
        {#if session.whatsNew.notes}
          <div class="wn-body md">{@html renderMarkdown(session.whatsNew.notes)}</div>
        {:else}
          <p class="wn-body muted">Updated to v{session.whatsNew.version}.</p>
        {/if}
        <div class="wn-foot">
          <button class="update-btn mono" onclick={() => session.dismissWhatsNew()}>
            got it
          </button>
        </div>
      </div>
    </div>
  {/if}

  {#if session.paletteOpen}
    <CommandPalette onclose={() => (session.paletteOpen = false)} />
  {/if}
  {#if session.quickOpen}
    <QuickOpen onclose={() => (session.quickOpen = false)} />
  {/if}
  <ContextMenu />
  <Notice />
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .topbar {
    display: flex;
    align-items: center;
    height: 36px;
    flex: none;
    /* light from above: a lit top edge over a slightly deeper base */
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--fg-0) 3%, var(--bg-1)),
      var(--bg-1)
    );
    border-bottom: 1px solid var(--border);
    box-shadow: inset 0 1px 0 var(--hairline);
    user-select: none;
  }
  .frame {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-0);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 48px;
    padding-left: 12px;
    flex: none;
    user-select: none;
    box-shadow: inset 0 1px 0 var(--hairline);
  }
  .bar .win-controls {
    align-self: flex-start;
    height: 36px;
  }
  .bar-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    color: var(--fg-2);
    flex: none;
  }
  .bar-btn:hover,
  .bar-btn.on {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .title {
    min-width: 0;
    max-width: 46ch;
    padding: 0 6px;
    font-size: 14px;
    font-weight: 600;
    color: var(--fg-0);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .content {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .mode-view {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
  }
  .win-controls {
    display: flex;
    align-self: stretch;
    margin-left: 4px;
  }
  .win-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 44px;
    color: var(--fg-2);
    transition:
      background var(--motion),
      color var(--motion);
  }
  .win-btn:hover {
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-3));
    color: var(--fg-0);
  }
  .win-btn.close:hover {
    background: var(--err);
    color: #fff;
  }
  .updatebar {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 12px;
    flex: none;
    font-size: 12px;
    color: var(--fg-1);
    background: var(--bg-1);
    border-bottom: 1px solid var(--border);
    border-left: 2px solid var(--accent);
    box-shadow:
      0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent),
      0 0 22px -6px color-mix(in srgb, var(--accent) 65%, transparent);
    animation: update-glow 3.4s ease-in-out infinite;
  }
  @keyframes update-glow {
    0%,
    100% {
      box-shadow:
        0 0 0 1px color-mix(in srgb, var(--accent) 35%, transparent),
        0 0 14px -6px color-mix(in srgb, var(--accent) 45%, transparent);
    }
    50% {
      box-shadow:
        0 0 0 1px color-mix(in srgb, var(--accent) 60%, transparent),
        0 0 26px -4px color-mix(in srgb, var(--accent) 75%, transparent);
    }
  }
  :global(:root[data-ambient-motion='off']) .updatebar {
    animation: none;
  }
  .wn-scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--bg-0) 72%, transparent);
    backdrop-filter: blur(2px);
  }
  .wn-card {
    width: min(520px, calc(100vw - 64px));
    max-height: min(70vh, 560px);
    display: flex;
    flex-direction: column;
    background: var(--bg-1);
    border: 1px solid color-mix(in srgb, var(--accent) 55%, var(--border));
    border-radius: 8px;
    box-shadow:
      0 0 0 1px color-mix(in srgb, var(--accent) 30%, transparent),
      0 0 40px -8px color-mix(in srgb, var(--accent) 55%, transparent),
      var(--shadow-lg);
  }
  .wn-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .wn-title {
    font-size: 12px;
    letter-spacing: 0.04em;
    color: var(--accent);
  }
  .wn-x {
    color: var(--fg-2);
    font-size: 12px;
    padding: 2px 6px;
  }
  .wn-x:hover {
    color: var(--fg-0);
  }
  .wn-body {
    padding: 14px;
    overflow-y: auto;
    font-size: 13px;
    line-height: 1.55;
    color: var(--fg-1);
  }
  .wn-body :global(ul) {
    margin: 6px 0;
    padding-left: 18px;
  }
  .wn-body :global(li) {
    margin: 3px 0;
  }
  .wn-body :global(code) {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--bg-3);
    padding: 1px 4px;
    border-radius: 3px;
  }
  .wn-foot {
    display: flex;
    justify-content: flex-end;
    padding: 10px 14px 14px;
  }
  .updatebar .dim {
    color: var(--fg-2);
  }
  .update-notes {
    color: var(--fg-2);
    cursor: help;
    border: 1px solid var(--border);
    border-radius: 50%;
    padding: 0 4px;
    margin-left: 4px;
    font-size: 10px;
  }
  .update-btn {
    color: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 2px 10px;
    font-size: 12px;
    transition: background var(--motion), color var(--motion);
  }
  .update-btn:hover {
    background: var(--accent);
    color: var(--bg-0);
  }
  .update-x {
    color: var(--fg-2);
    font-size: 11px;
    padding: 2px 6px;
  }
  .update-x:hover {
    color: var(--fg-0);
  }
  .win-btn.close:hover {
    background: var(--err);
    color: #fff;
  }
  .spacer {
    flex: 1;
  }
</style>
