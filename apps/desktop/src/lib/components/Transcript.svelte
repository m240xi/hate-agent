<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../store.svelte';
  import TranscriptBlockView from './TranscriptBlock.svelte';
  import ToolGroup from './ToolGroup.svelte';
  import ImageStrip from './ImageStrip.svelte';
  import LiveStatus from './LiveStatus.svelte';
  import { imageSources } from '../images';
  import type { TranscriptBlock } from '../types';

  let scrollEl = $state<HTMLDivElement | null>(null);
  let pinned = $state(true);
  let showJump = $state(false);
  let announce = $state('');

  const VIRTUALIZE_AT = 200;
  let revealExtra = $state(0);
  const visibleBlocks = $derived(
    session.blocks.length > VIRTUALIZE_AT + revealExtra
      ? session.blocks.slice(session.blocks.length - VIRTUALIZE_AT - revealExtra)
      : session.blocks,
  );
  const hiddenCount = $derived(session.blocks.length - visibleBlocks.length);

  type Row =
    | { kind: 'block'; key: string; block: TranscriptBlock }
    | { kind: 'tools'; key: string; blocks: TranscriptBlock[] }
    | { kind: 'images'; key: string; sources: string[] };

  /** Group runs of tool cards and hoist a turn's generated images into one strip. */
  const rows = $derived.by(() => {
    const out: Row[] = [];
    let run: TranscriptBlock[] = [];
    let images: string[] = [];

    const flushRun = (): void => {
      if (run.length >= 3) {
        out.push({ kind: 'tools', key: `g${run[0]?.id}`, blocks: run });
      } else {
        for (const b of run) out.push({ kind: 'block', key: `b${b.id}`, block: b });
      }
      run = [];
    };
    const flushImages = (): void => {
      const unique = [...new Set(images)];
      if (unique.length > 0) out.push({ kind: 'images', key: `i${out.length}`, sources: unique });
      images = [];
    };

    for (const b of visibleBlocks) {
      if (b.kind === 'tool') {
        run.push(b);
        if (b.ok !== undefined) images.push(...imageSources(b.call?.args, b.preview, b.images));
        continue;
      }
      flushRun();
      if (b.kind === 'user') {
        flushImages();
        out.push({ kind: 'block', key: `b${b.id}`, block: b });
      } else if (b.kind === 'image') {
        images.push(...(b.images ?? []));
      } else {
        out.push({ kind: 'block', key: `b${b.id}`, block: b });
      }
    }
    flushRun();
    flushImages();
    return out;
  });

  // Announce settled results only, so streamed text isn't read token by token.
  $effect(() => {
    const last = session.blocks.at(-1);
    if (!last) return;
    if (last.kind === 'error') {
      announce = last.text ?? 'error';
    } else if (last.kind === 'text' && !last.streaming && !last.local) {
      const text = (last.text ?? '').replace(/\s+/g, ' ').trim();
      announce = text ? `Assistant replied: ${text.slice(0, 240)}` : '';
    }
  });

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
  <p class="sr-only" role="status" aria-live="polite">{announce}</p>
  <div class="transcript" bind:this={scrollEl} onscroll={onScroll} data-ctx="transcript">
    {#if hiddenCount > 0}
      <div class="earlier">
        {hiddenCount} earlier blocks hidden
        <button onclick={revealEarlier}>show</button>
      </div>
    {/if}
    {#each rows as row (row.key)}
      {#if row.kind === 'block'}
        <TranscriptBlockView block={row.block} />
      {:else if row.kind === 'tools'}
        <ToolGroup blocks={row.blocks} />
      {:else}
        <ImageStrip sources={row.sources} />
      {/if}
    {/each}
    <LiveStatus />
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
    padding: 16px max(24px, calc((100% - 768px) / 2)) 12px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    overflow-x: hidden;
  }
  .transcript > :global(*) {
    flex-shrink: 0;
    min-width: 0;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
    border: 0;
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
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    padding: 5px 12px;
    font-size: 11.5px;
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-pill);
    box-shadow: var(--el-2);
    color: var(--fg-1);
  }
  .jump:hover {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
</style>
