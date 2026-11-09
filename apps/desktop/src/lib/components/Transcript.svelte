<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../store.svelte';
  import TranscriptBlock from './TranscriptBlock.svelte';

  let scrollEl = $state<HTMLDivElement | null>(null);
  let pinned = $state(true);
  let showJump = $state(false);

  const VIRTUALIZE_AT = 200;
  let revealExtra = $state(0);
  const visibleBlocks = $derived(
    session.blocks.length > VIRTUALIZE_AT + revealExtra
      ? session.blocks.slice(session.blocks.length - VIRTUALIZE_AT - revealExtra)
      : session.blocks,
  );
  const hiddenCount = $derived(session.blocks.length - visibleBlocks.length);

  function revealEarlier(): void {
    revealExtra += VIRTUALIZE_AT;
  }

  function onScroll(): void {
    if (!scrollEl) return;
    const dist = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
    pinned = dist < 40;
    showJump = !pinned;
  }

  function jumpToBottom(): void {
    pinned = true;
    scrollEl?.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
  }

  $effect(() => {
    session.blocks.length;
    session.blocks.at(-1)?.text;
    if (pinned && scrollEl) {
      untrack(() => {
        scrollEl!.scrollTop = scrollEl!.scrollHeight;
      });
    }
  });
</script>

<div class="transcript-wrap">
  <div class="transcript" bind:this={scrollEl} onscroll={onScroll} aria-live="polite">
    {#if hiddenCount > 0}
      <div class="earlier">
        {hiddenCount} earlier blocks hidden
        <button onclick={revealEarlier}>show</button>
      </div>
    {/if}
    {#each visibleBlocks as block (block.id)}
      <TranscriptBlock {block} />
    {/each}
    {#if session.blocks.length === 0}
      <div class="empty-hint">Send a message to start a session.</div>
    {/if}
  </div>
  {#if showJump}
    <button class="jump" onclick={jumpToBottom} aria-label="Jump to latest">
      ↓ latest
    </button>
  {/if}
</div>

<style>
  .transcript-wrap {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .transcript {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px 8px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    overflow-x: hidden;
  }
  .transcript > :global(*) {
    flex-shrink: 0;
    min-width: 0;
  }
  .earlier {
    align-self: center;
    font-size: 11px;
    color: var(--fg-3);
    padding: 4px 10px;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
  }
  .earlier button {
    color: var(--accent);
    margin-left: 6px;
  }
  .empty-hint {
    color: var(--fg-3);
    text-align: center;
    margin-top: 40px;
  }
  .jump {
    position: absolute;
    bottom: 10px;
    right: 16px;
    padding: 4px 10px;
    font-size: 11px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--fg-1);
  }
  .jump:hover {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
</style>
