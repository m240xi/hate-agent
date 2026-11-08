<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from './lib/store.svelte';
  import Pane from './lib/components/Pane.svelte';
  import FileTree from './lib/components/FileTree.svelte';
  import Transcript from './lib/components/Transcript.svelte';
  import Composer from './lib/components/Composer.svelte';
  import PermissionBar from './lib/components/PermissionBar.svelte';
  import Inspector from './lib/components/Inspector.svelte';
  import EmptyState from './lib/components/EmptyState.svelte';
  import Icon from './lib/components/Icon.svelte';

  onMount(() => {
    void session.start();
    return () => session.destroy();
  });

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
</script>

<div class="shell">
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
    <header class="topbar">
      <span class="brand">hate-agent</span>
      <span class="state mono" class:running={session.running}>
        {session.sessionState}
      </span>
      <span class="spacer"></span>
      <button
        class="icon-btn"
        onclick={() => session.clearTranscript()}
        aria-label="Clear transcript"
        title="Clear transcript"
      >
        <Icon name="x" size={13} />
      </button>
    </header>

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
</div>

<style>
  .shell {
    display: flex;
    height: 100vh;
    overflow: hidden;
  }
  .center {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-0);
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 32px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
    flex: none;
    background: var(--bg-1);
  }
  .brand {
    font-size: 12px;
    font-weight: 700;
    letter-spacing: -0.01em;
    color: var(--fg-0);
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
    width: 22px;
    height: 22px;
    border-radius: var(--radius);
    color: var(--fg-2);
  }
  .icon-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
</style>
