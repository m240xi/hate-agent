<script lang="ts">
  // Segmented mode switch — sits in the topbar on every non-landing
  // surface. Home is reached via the wordmark.
  import { session, type AppMode } from '../store.svelte';
  import Icon from './Icon.svelte';

  const ITEMS: { mode: AppMode; icon: string; label: string }[] = [
    { mode: 'code', icon: 'code', label: 'Code' },
    { mode: 'chat', icon: 'chat', label: 'Chat' },
    { mode: 'settings', icon: 'gear', label: 'Settings' },
  ];
</script>

<nav class="modes" aria-label="Mode">
  {#each ITEMS as item (item.mode)}
    <button
      class="mode"
      class:active={session.mode === item.mode}
      onclick={() => session.setMode(item.mode)}
      aria-pressed={session.mode === item.mode}
    >
      <Icon name={item.icon} size={13} />
      {item.label}
    </button>
  {/each}
</nav>

<style>
  .modes {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
  }
  .mode {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--fg-2);
    border-radius: var(--radius-pill);
    transition:
      background var(--motion),
      color var(--motion);
  }
  .mode:hover {
    color: var(--fg-0);
  }
  .mode.active {
    background: var(--bg-0);
    color: var(--fg-0);
    box-shadow: var(--shadow-sm);
  }
</style>
