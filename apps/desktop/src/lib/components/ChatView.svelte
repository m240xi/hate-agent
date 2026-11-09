<script lang="ts">
  // Chat — the same session backend with chat-only chrome: a centered
  // reading column, no workspace panes. Code and Chat share the wire;
  // the split is presentation plus tool gating — chat sessions spawn
  // with no workspace tools (MCP servers still apply).
  import { session } from '../store.svelte';
  import Transcript from './Transcript.svelte';
  import Composer from './Composer.svelte';
  import PermissionBar from './PermissionBar.svelte';
  import EmptyState from './EmptyState.svelte';
  import SessionBrowser from './SessionBrowser.svelte';
  import Pane from './Pane.svelte';
  import Inspector from './Inspector.svelte';
</script>

<div class="chat">
  <div class="chat-inner">
    {#if session.pickerMode === 'chat' || session.sessionBusy}
      {#if session.sessionBusy}
        <div class="chat-empty"><p class="busy-line">opening session…</p></div>
      {:else}
        <SessionBrowser mode="chat" />
      {/if}
    {:else if session.blocks.length === 0}
      <div class="chat-empty">
        <EmptyState />
      </div>
    {:else}
      <Transcript />
    {/if}
    {#if session.pickerMode !== 'chat' && !session.sessionBusy}
      <PermissionBar />
      <Composer />
    {/if}
  </div>

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
  .chat {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    background: var(--bg-0);
  }
  .chat-inner {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    width: 100%;
    max-width: 740px;
    margin: 0 auto;
  }
  .chat-empty {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
  }
  .chat-empty :global(.empty) {
    margin: auto;
    padding-bottom: 8vh;
  }
  .busy-line {
    margin: auto;
    font-size: 12px;
    color: var(--fg-3);
  }
</style>
