<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { fade } from 'svelte/transition';
  import { session } from './lib/store.svelte';
  import Pane from './lib/components/Pane.svelte';
  import FileTree from './lib/components/FileTree.svelte';
  import FileViewer from './lib/components/FileViewer.svelte';
  import Transcript from './lib/components/Transcript.svelte';
  import Composer from './lib/components/Composer.svelte';
  import PermissionBar from './lib/components/PermissionBar.svelte';
  import Inspector from './lib/components/Inspector.svelte';
  import EmptyState from './lib/components/EmptyState.svelte';
  import Landing from './lib/components/Landing.svelte';
  import ChatView from './lib/components/ChatView.svelte';
  import SettingsView from './lib/components/SettingsView.svelte';
  import ModeSwitch from './lib/components/ModeSwitch.svelte';
  import SessionBrowser from './lib/components/SessionBrowser.svelte';
  import Icon from './lib/components/Icon.svelte';
  import { renderMarkdown } from './lib/markdown';

  let inTauri = $state(false);

  onMount(() => {
    inTauri = '__TAURI_INTERNALS__' in window;
    void session.start();
    return () => session.destroy();
  });

  async function onTopbarDown(e: MouseEvent): Promise<void> {
    if (!inTauri || e.button !== 0) return;
    const el = e.target as HTMLElement;
    if (el.closest('button, a, input, textarea, select, [role="button"]')) return;
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const win = getCurrentWindow();
    if (e.detail === 2) {
      await win.toggleMaximize();
    } else {
      await win.startDragging();
    }
  }

  async function winMin(): Promise<void> {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().minimize();
  }
  async function winMax(): Promise<void> {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().toggleMaximize();
  }
  async function winClose(): Promise<void> {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().close();
  }

  function onWindowKeydown(e: KeyboardEvent): void {
    if (e.key !== 'Escape' || !session.viewer) return;
    const el = e.target as HTMLElement | null;
    if (el?.closest('input, textarea, select, [contenteditable="true"]')) return;
    e.preventDefault();
    session.closeViewer();
  }

  $effect(() => {
    localStorage.setItem('hate:pane:left', String(session.leftWidth));
    localStorage.setItem('hate:pane:right', String(session.rightWidth));
    localStorage.setItem('hate:pane:leftCollapsed', String(session.leftCollapsed));
    localStorage.setItem('hate:pane:rightCollapsed', String(session.rightCollapsed));
  });

  $effect(() => {
    const l = Number(localStorage.getItem('hate:pane:left'));
    const r = Number(localStorage.getItem('hate:pane:right'));
    if (l) session.leftWidth = l;
    if (r) session.rightWidth = r;
    session.leftCollapsed = localStorage.getItem('hate:pane:leftCollapsed') === 'true';
    session.rightCollapsed = localStorage.getItem('hate:pane:rightCollapsed') === 'true';
  });

  const workspaceName = $derived(
    session.workspacePath?.split(/[\\/]/).filter(Boolean).pop() ?? null,
  );

  $effect(() => {
    const m = session.mode;
    untrack(() => {
      if (m === 'chat' || m === 'code') void session.enterMode(m);
      if (m !== 'code') session.closeViewer();
    });
  });
</script>

<svelte:window onkeydown={onWindowKeydown} />

<div class="shell">
  <header class="topbar" role="presentation" onmousedown={onTopbarDown}>
    <button class="brand" onclick={() => session.setMode('home')} title="Home">
      hate-agent
    </button>
    <ModeSwitch />
    {#if session.mode === 'code'}
      <button
        class="ws-chip mono"
        class:set={!!session.workspacePath}
        onclick={() => session.pickWorkspace()}
        title={session.workspacePath ?? 'Attach a folder for code sessions'}
      >
        <Icon name="folder" size={12} />
        {workspaceName ?? 'Attach folder'}
      </button>
    {/if}
    <span class="spacer"></span>
    {#if session.mode === 'code' || session.mode === 'chat'}
      <button
        class="sess-btn mono"
        onclick={() =>
          (session.pickerMode = session.mode === 'code' ? 'code' : 'chat')}
        title="Sessions — browse history or start a new one"
      >
        <Icon name="clock" size={12} />
        <span class="sess-title">{session.sessionMeta?.title || 'Sessions'}</span>
      </button>
      <span class="state mono" class:running={session.running}>
        {session.sessionState}
      </span>
    {/if}
    {#if inTauri}
      <div class="win-controls">
        <button class="win-btn" onclick={winMin} aria-label="Minimize" title="Minimize">
          <Icon name="min" size={11} />
        </button>
        <button class="win-btn" onclick={winMax} aria-label="Maximize" title="Maximize">
          <Icon name="max" size={11} />
        </button>
        <button
          class="win-btn close"
          onclick={winClose}
          aria-label="Close"
          title="Close"
        >
          <Icon name="x" size={12} />
        </button>
      </div>
    {/if}
  </header>

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

  <div class="body">
    {#key session.mode}
      <div class="mode-view" in:fade={{ duration: 140 }}>
        {#if session.mode === 'home'}
          <Landing />
        {:else if session.mode === 'chat'}
          <ChatView />
        {:else if session.mode === 'settings'}
          <SettingsView />
        {:else}
          <Pane
            side="left"
            title="Files"
            collapsed={session.leftCollapsed}
            width={session.leftWidth}
            onToggle={() => (session.leftCollapsed = !session.leftCollapsed)}
            onResize={(w) => (session.leftWidth = w)}
          >
            <FileTree />
          </Pane>

          <main class="center">
            {#if session.viewer}
              <FileViewer />
            {:else if session.pickerMode === 'code' || session.sessionBusy}
              {#if session.sessionBusy}
                <div class="center-busy"><p>opening session…</p></div>
              {:else}
                <SessionBrowser mode="code" />
              {/if}
            {:else if session.blocks.length === 0}
              <EmptyState />
            {:else}
              <Transcript />
            {/if}

            {#if session.pickerMode !== 'code' && !session.sessionBusy}
              <PermissionBar />
              <Composer />
            {/if}
          </main>

          <Pane
            side="right"
            title="Inspector"
            collapsed={session.rightCollapsed}
            width={session.rightWidth}
            onToggle={() => (session.rightCollapsed = !session.rightCollapsed)}
            onResize={(w) => (session.rightWidth = w)}
          >
            <Inspector />
          </Pane>
        {/if}
      </div>
    {/key}
  </div>
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
  .mode-view {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 0 0 12px;
    border-bottom: 1px solid var(--border);
    flex: none;
    background: var(--bg-1);
    user-select: none;
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
    background: var(--bg-3);
    color: var(--fg-0);
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
  @media (prefers-reduced-motion: reduce) {
    .updatebar {
      animation: none;
    }
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
      0 18px 40px -20px #000;
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
  .brand {
    font-size: 13px;
    font-weight: 800;
    letter-spacing: -0.02em;
    color: var(--fg-0);
    padding: 4px 6px;
    border-radius: var(--radius-sm);
  }
  .brand:hover {
    background: var(--bg-2);
  }
  .ws-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 220px;
    padding: 4px 10px;
    font-size: 11px;
    color: var(--fg-3);
    background: var(--bg-2);
    border: 1px dashed var(--border-strong);
    border-radius: var(--radius-pill);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .ws-chip.set {
    color: var(--fg-1);
    border-style: solid;
    border-color: var(--border);
  }
  .ws-chip:hover {
    color: var(--fg-0);
    border-color: var(--accent-dim);
  }
  .center {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-0);
  }
  .state {
    font-size: 10.5px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .sess-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 260px;
    padding: 4px 10px;
    font-size: 11px;
    color: var(--fg-2);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    overflow: hidden;
    white-space: nowrap;
  }
  .sess-btn:hover {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .sess-title {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .center-busy {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .center-busy p {
    font-size: 12px;
    color: var(--fg-3);
  }
  .state.running {
    color: var(--accent);
  }
  .spacer {
    flex: 1;
  }
</style>
