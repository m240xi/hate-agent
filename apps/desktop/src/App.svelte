<script lang="ts">
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import { session } from './lib/store.svelte';
  import Pane from './lib/components/Pane.svelte';
  import FileTree from './lib/components/FileTree.svelte';
  import Transcript from './lib/components/Transcript.svelte';
  import Composer from './lib/components/Composer.svelte';
  import PermissionBar from './lib/components/PermissionBar.svelte';
  import Inspector from './lib/components/Inspector.svelte';
  import EmptyState from './lib/components/EmptyState.svelte';
  import Landing from './lib/components/Landing.svelte';
  import ChatView from './lib/components/ChatView.svelte';
  import SettingsView from './lib/components/SettingsView.svelte';
  import ModeSwitch from './lib/components/ModeSwitch.svelte';
  import Icon from './lib/components/Icon.svelte';

  let inTauri = $state(false);

  onMount(() => {
    inTauri = '__TAURI_INTERNALS__' in window;
    void session.start();
    return () => session.destroy();
  });

  // Custom titlebar: press-and-drag on empty bar space drags the window;
  // double-click toggles maximize. Interactive children are exempt via
  // the closest() check, so no Tauri drag-region attribute is needed.
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

  // Persist pane sizes per workspace (spec §20); keys land with real
  // workspace identity in M1 — localStorage is the M0 stand-in.
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
</script>

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
      <span class="state mono" class:running={session.running}>
        {session.sessionState}
      </span>
      <button
        class="icon-btn"
        onclick={() => session.clearTranscript()}
        aria-label="Clear transcript"
        title="Clear transcript"
      >
        <Icon name="x" size={13} />
      </button>
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
            {#if session.blocks.length === 0}
              <EmptyState />
            {:else}
              <Transcript />
            {/if}

            <PermissionBar />
            <Composer />
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
  .state.running {
    color: var(--accent);
  }
  .spacer {
    flex: 1;
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: var(--radius-sm);
    color: var(--fg-2);
  }
  .icon-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
</style>
