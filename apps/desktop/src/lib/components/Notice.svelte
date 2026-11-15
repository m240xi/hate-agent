<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  let timer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const notice = session.notice;
    if (timer) clearTimeout(timer);
    if (!notice) return;
    timer = setTimeout(() => session.dismissNotice(), 12_000);
    return () => {
      if (timer) clearTimeout(timer);
    };
  });
</script>

{#if session.notice}
  <div class="toast rise" role="status">
    <button class="body" onclick={() => void session.focusWindow()}>
      <Icon name="check" size={12} />
      <span>{session.notice.text}</span>
    </button>
    <button class="close" onclick={() => session.dismissNotice()} aria-label="Dismiss">
      <Icon name="x" size={11} />
    </button>
  </div>
{/if}

<style>
  .toast {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 70;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px;
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
  }
  .body {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
    font-size: 12px;
    color: var(--fg-0);
  }
  .body:hover {
    color: var(--accent);
  }
  .close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    color: var(--fg-3);
  }
  .close:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
</style>
