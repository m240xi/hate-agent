<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type { FileNode } from '../types';

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  function flatten(nodes: FileNode[], prefix = '', out: string[] = []): string[] {
    for (const n of nodes) {
      const p = prefix ? `${prefix}/${n.name}` : n.name;
      if (n.kind === 'file') out.push(p);
      if (n.children) flatten(n.children, p, out);
    }
    return out;
  }

  const allPaths = $derived(flatten(session.tree));
  let query = $state('');
  let sel = $state(0);
  let filterEl = $state<HTMLInputElement | null>(null);

  onMount(() => filterEl?.focus());

  const results = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return allPaths.slice(0, 15);
    return allPaths.filter((p) => p.toLowerCase().includes(q)).slice(0, 15);
  });

  function pick(path: string): void {
    session.draft = `${session.draft}@${path} `;
    onclose();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      sel = Math.min(sel + 1, results.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      sel = Math.max(sel - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const p = results[sel];
      if (p) pick(p);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={(e) => {
    if (e.target === e.currentTarget) onclose();
  }}
  onkeydown={onKeydown}
  role="presentation"
>
  <div class="picker rise" role="dialog" aria-label="Attach file" tabindex="-1">
    <div class="filter-wrap">
      <Icon name="file" size={13} />
      <input
        class="filter"
        placeholder="File path…"
        bind:value={query}
        onkeydown={onKeydown}
        aria-label="Filter files"
        bind:this={filterEl}
      />
    </div>
    <ul class="list" role="listbox">
      {#each results as p, i}
        <li>
          <button class="path" class:sel={i === sel} onclick={() => pick(p)} onmouseenter={() => (sel = i)} role="option" aria-selected={i === sel}>
            <span class="mono">@{p}</span>
          </button>
        </li>
      {/each}
      {#if results.length === 0}
        <li class="none">no matching files</li>
      {/if}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 15vh;
    z-index: 50;
  }
  .picker {
    width: 460px;
    max-width: 90vw;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 8px 30px rgb(0 0 0 / 0.4);
    overflow: hidden;
  }
  .filter-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-2);
  }
  .filter {
    flex: 1;
    background: none;
    border: none;
    outline: none;
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--fg-0);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 300px;
    overflow: auto;
  }
  .path {
    display: block;
    width: 100%;
    height: 26px;
    padding: 0 10px;
    border-radius: var(--radius);
    text-align: left;
    font-size: 12px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path.sel {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .none {
    padding: 12px;
    text-align: center;
    color: var(--fg-3);
    font-size: 12px;
  }
</style>
