<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { session } from '../store.svelte';
  import TranscriptBlockView from './TranscriptBlock.svelte';
  import ToolGroup from './ToolGroup.svelte';
  import ImageStrip from './ImageStrip.svelte';
  import LiveStatus from './LiveStatus.svelte';
  import Icon from './Icon.svelte';
  import { imageSources } from '../images';
  import type { TranscriptBlock } from '../types';

  let scrollEl = $state<HTMLDivElement | null>(null);
  let pinned = $state(true);
  let showJump = $state(false);
  let announce = $state('');

  // Search & Jump Rail state
  let searchQuery = $state('');
  let matchCursor = $state(0);
  let activeSearchBlockId = $state<number | null>(null);
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let railOpen = $state(false);
  let railFilter = $state<'all' | 'starred'>('all');

  const VIRTUALIZE_AT = 200;
  let revealExtra = $state(0);
  const visibleBlocks = $derived(
    session.blocks.length > VIRTUALIZE_AT + revealExtra
      ? session.blocks.slice(session.blocks.length - VIRTUALIZE_AT - revealExtra)
      : session.blocks,
  );
  const hiddenCount = $derived(session.blocks.length - visibleBlocks.length);

  function blockSearchText(b: TranscriptBlock): string {
    if (b.text) return b.text;
    if (b.call) {
      return `${b.call.name} ${JSON.stringify(b.call.args)} ${b.preview ?? ''}`;
    }
    return '';
  }

  const searchMatches = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return [] as { blockId: number; index: number }[];
    const out: { blockId: number; index: number }[] = [];
    session.blocks.forEach((b, index) => {
      if (blockSearchText(b).toLowerCase().includes(q)) {
        out.push({ blockId: b.id, index });
      }
    });
    return out;
  });

  const jumpEntries = $derived.by(() => {
    const out: { blockId: number; index: number; turn: number; kind: 'user' | 'star'; preview: string; bookmarked: boolean }[] = [];
    let turn = 0;
    session.blocks.forEach((b, index) => {
      if (b.kind === 'user') {
        turn += 1;
        out.push({
          blockId: b.id,
          index,
          turn,
          kind: 'user',
          preview: (b.text ?? '').replace(/\s+/g, ' ').trim().slice(0, 72) || '(image)',
          bookmarked: !!b.bookmarked,
        });
      } else if (b.bookmarked) {
        out.push({
          blockId: b.id,
          index,
          turn,
          kind: 'star',
          preview: (b.text ?? b.call?.name ?? 'bookmarked block').replace(/\s+/g, ' ').trim().slice(0, 72),
          bookmarked: true,
        });
      }
    });
    return railFilter === 'starred' ? out.filter((e) => e.bookmarked) : out;
  });

  const bookmarkCount = $derived(session.blocks.filter((b) => b.bookmarked).length);
  const promptCount = $derived(session.blocks.filter((b) => b.kind === 'user').length);

  $effect(() => {
    if (session.transcriptSearchOpen) {
      void tick().then(() => {
        searchInputEl?.focus();
        searchInputEl?.select();
      });
    } else {
      activeSearchBlockId = null;
    }
  });

  $effect(() => {
    void searchQuery;
    matchCursor = 0;
    if (searchMatches.length > 0) {
      void jumpToBlock(searchMatches[0]!.blockId, searchMatches[0]!.index, true);
    } else {
      activeSearchBlockId = null;
    }
  });

  async function jumpToBlock(blockId: number, blockIndex: number, markSearch = false): Promise<void> {
    const needed = session.blocks.length - blockIndex;
    if (needed > VIRTUALIZE_AT + revealExtra) {
      revealExtra = needed - VIRTUALIZE_AT + 10;
    }
    await tick();
    pinned = false;
    const el = scrollEl?.querySelector<HTMLElement>(`[data-block="${blockId}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: 'smooth' });
      if (markSearch) activeSearchBlockId = blockId;
    }
  }

  function stepMatch(delta: 1 | -1): void {
    if (searchMatches.length === 0) return;
    matchCursor = (matchCursor + delta + searchMatches.length) % searchMatches.length;
    const target = searchMatches[matchCursor];
    if (target) void jumpToBlock(target.blockId, target.index, true);
  }

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

  <div class="float-tools">
    {#if session.transcriptSearchOpen}
      <div class="search-bar pop-panel" role="search" aria-label="Search transcript">
        <Icon name="search" size={12} />
        <input
          class="s-input mono"
          placeholder="Find in chat…"
          bind:value={searchQuery}
          bind:this={searchInputEl}
          onkeydown={(e) => {
            if (e.key === 'Escape') {
              e.preventDefault();
              session.transcriptSearchOpen = false;
            } else if (e.key === 'Enter') {
              e.preventDefault();
              stepMatch(e.shiftKey ? -1 : 1);
            }
          }}
        />
        <span class="s-count mono">
          {searchMatches.length > 0 ? `${matchCursor + 1}/${searchMatches.length}` : searchQuery.trim() ? '0/0' : ''}
        </span>
        <button class="s-btn" onclick={() => stepMatch(-1)} disabled={searchMatches.length === 0} title="Previous match (Shift+Enter)" aria-label="Previous match">
          <Icon name="chevron-up" size={12} />
        </button>
        <button class="s-btn" onclick={() => stepMatch(1)} disabled={searchMatches.length === 0} title="Next match (Enter)" aria-label="Next match">
          <Icon name="chevron-down" size={12} />
        </button>
        <button class="s-btn" onclick={() => (session.transcriptSearchOpen = false)} title="Close search (Esc)" aria-label="Close search">
          <Icon name="x" size={12} />
        </button>
      </div>
    {:else if promptCount > 0}
      <div class="rail-controls">
        <button
          class="rail-pill mono"
          onclick={() => (session.transcriptSearchOpen = true)}
          title="Search transcript (Ctrl+F)"
        >
          <Icon name="search" size={11} />
        </button>
        <button
          class="rail-pill mono"
          class:on={railOpen}
          onclick={() => (railOpen = !railOpen)}
          title="Prompt navigator & bookmarks"
        >
          <Icon name="list-ordered" size={11} />
          <span>{promptCount}</span>
          {#if bookmarkCount > 0}
            <span class="star-cnt">★{bookmarkCount}</span>
          {/if}
        </button>
      </div>
    {/if}

    {#if railOpen && !session.transcriptSearchOpen}
      <div class="jump-pop pop-panel" role="dialog" aria-label="Prompt navigator">
        <div class="jp-head">
          <button class="jp-tab mono" class:on={railFilter === 'all'} onclick={() => (railFilter = 'all')}>
            Prompts ({promptCount})
          </button>
          <button class="jp-tab mono" class:on={railFilter === 'starred'} onclick={() => (railFilter = 'starred')}>
            ★ Starred ({bookmarkCount})
          </button>
          <span class="spacer"></span>
          <button class="s-btn" onclick={() => (railOpen = false)} aria-label="Close navigator">
            <Icon name="x" size={11} />
          </button>
        </div>
        <ul class="jp-list">
          {#each jumpEntries as entry (`${entry.blockId}:${entry.kind}`)}
            <li>
              <button
                class="jp-item"
                onclick={() => {
                  railOpen = false;
                  void jumpToBlock(entry.blockId, entry.index, false);
                }}
              >
                <span class="jp-num mono">{entry.kind === 'star' ? '★' : `#${entry.turn}`}</span>
                <span class="jp-text">{entry.preview}</span>
              </button>
            </li>
          {/each}
          {#if jumpEntries.length === 0}
            <li class="jp-empty">No {railFilter === 'starred' ? 'starred messages' : 'prompts'} yet.</li>
          {/if}
        </ul>
      </div>
    {/if}
  </div>

  <div class="transcript" bind:this={scrollEl} onscroll={onScroll} data-ctx="transcript">
    {#if hiddenCount > 0}
      <div class="earlier">
        {hiddenCount} earlier blocks hidden
        <button onclick={revealEarlier}>show</button>
      </div>
    {/if}
    {#each rows as row (row.key)}
      {#if row.kind === 'block'}
        <div class="block-slot" class:search-hit={activeSearchBlockId === row.block.id}>
          <TranscriptBlockView block={row.block} />
        </div>
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
  .float-tools {
    position: absolute;
    top: 8px;
    right: 16px;
    z-index: 20;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 6px;
    pointer-events: none;
  }
  .float-tools > * {
    pointer-events: auto;
  }
  .rail-controls {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .rail-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 8px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-1) 88%, transparent);
    color: var(--fg-2);
    font-size: 10.5px;
    box-shadow: var(--el-1);
  }
  .rail-pill:hover,
  .rail-pill.on {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .star-cnt {
    color: var(--warn);
  }
  .search-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-1);
    box-shadow: var(--el-3);
    color: var(--fg-2);
  }
  .s-input {
    width: 170px;
    background: none;
    border: none;
    outline: none;
    font-size: 12px;
    color: var(--fg-0);
  }
  .s-count {
    font-size: 10px;
    color: var(--fg-3);
    min-width: 28px;
    text-align: right;
  }
  .s-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    color: var(--fg-2);
  }
  .s-btn:hover:not(:disabled) {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .s-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .jump-pop {
    width: 300px;
    max-width: 85vw;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-1);
    box-shadow: var(--el-3);
    overflow: hidden;
  }
  .jp-head {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  .jp-tab {
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: 5px;
    color: var(--fg-2);
  }
  .jp-tab.on {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .jp-list {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 280px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .jp-item {
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 5px 8px;
    border-radius: 6px;
    text-align: left;
    font-size: 11.5px;
    color: var(--fg-1);
  }
  .jp-item:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .jp-num {
    flex: none;
    font-size: 10px;
    color: var(--accent);
  }
  .jp-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .jp-empty {
    padding: 12px;
    text-align: center;
    font-size: 11px;
    color: var(--fg-3);
  }
  .spacer {
    flex: 1;
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
  .block-slot {
    display: flex;
    flex-direction: column;
  }
  .block-slot.search-hit {
    border-radius: 12px;
    outline: 2px solid color-mix(in srgb, var(--accent) 65%, transparent);
    outline-offset: 4px;
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
