<script lang="ts">
  import Icon from './Icon.svelte';
  import ToolCard from './ToolCard.svelte';
  import type { TranscriptBlock } from '../types';

  let { blocks }: { blocks: TranscriptBlock[] } = $props();

  let open = $state(false);

  const summary = $derived.by(() => {
    const groups = new Map<string, number>();
    for (const b of blocks) {
      const name = b.call?.name ?? 'tool';
      groups.set(name, (groups.get(name) ?? 0) + 1);
    }
    return [...groups.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([name, n]) => (n === 1 ? name : `${n}× ${name}`))
      .slice(0, 3)
      .join(', ');
  });

  const failed = $derived(blocks.filter((b) => b.ok === false).length);
  const pending = $derived(blocks.filter((b) => b.ok === undefined).length);
  const stateOf = (ok: boolean | undefined) => (ok === undefined ? 'run' : ok ? 'ok' : 'fail');
</script>

<section class="tool-group" class:open>
  <button class="grp-head" onclick={() => (open = !open)} aria-expanded={open}>
    <span class="chev">{open ? '▾' : '▸'}</span>
    <Icon name="terminal" size={12} />
    <span class="count mono">{blocks.length} tools</span>
    <span class="detail mono">{summary}</span>
    <span class="strip" aria-hidden="true">
      {#each blocks as b (b.id)}<span class="tick" data-state={stateOf(b.ok)}></span>{/each}
    </span>
    {#if failed > 0}<span class="badge err mono">{failed} failed</span>{/if}
    {#if pending > 0}<span class="badge mono">{pending} running</span>{/if}
  </button>
  {#if open}
    <div class="grp-body">
      {#each blocks as block, i (block.id)}
        <ToolCard {block} index={i} />
      {/each}
    </div>
  {/if}
</section>

<style>
  .tool-group {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-1);
    overflow: hidden;
  }
  .grp-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 26px;
    padding: 0 8px;
    text-align: left;
    color: var(--fg-1);
  }
  .grp-head:hover {
    background: var(--bg-2);
  }
  .chev {
    width: 10px;
    font-size: 10px;
    color: var(--fg-3);
  }
  .count {
    flex: none;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--fg-0);
  }
  /* One tick per call: a 12-tool group reads at a glance — where it failed,
     where it is still working. */
  .strip {
    display: flex;
    align-items: center;
    gap: 1px;
    height: 9px;
    flex: none;
    overflow: hidden;
    max-width: 96px;
  }
  .tick {
    width: 2px;
    height: 100%;
    border-radius: 1px;
    flex: none;
    background: var(--border-strong);
  }
  .tick[data-state='ok'] {
    background: color-mix(in srgb, var(--ok) 70%, transparent);
  }
  .tick[data-state='run'] {
    background: color-mix(in srgb, var(--accent) 70%, transparent);
  }
  .tick[data-state='fail'] {
    background: var(--err);
  }
  .detail {
    flex: 1 1 0;
    min-width: 0;
    font-size: 11px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    flex: none;
    padding: 1px 6px;
    font-size: 10px;
    color: var(--fg-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
  }
  .badge.err {
    color: var(--err);
    border-color: color-mix(in srgb, var(--err) 40%, var(--border));
  }
  .grp-body {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    border-top: 1px solid var(--border);
    background: var(--bg-0);
  }
</style>
