<script lang="ts">
  // Chat — the same session backend with chat-only chrome: a centered
  // reading column, no workspace panes. (M1 has no tools yet, so Code
  // and Chat share the wire; the split is presentation until M2 lands
  // the tool surface and chat gets a tools-off request flag.)
  import { session } from '../store.svelte';
  import Transcript from './Transcript.svelte';
  import Composer from './Composer.svelte';
  import PermissionBar from './PermissionBar.svelte';
  import EmptyState from './EmptyState.svelte';
</script>

<div class="chat">
  <div class="chat-inner">
    {#if session.blocks.length === 0}
      <div class="chat-empty">
        <EmptyState />
      </div>
    {:else}
      <Transcript />
    {/if}
    <PermissionBar />
    <Composer />
  </div>
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
</style>
