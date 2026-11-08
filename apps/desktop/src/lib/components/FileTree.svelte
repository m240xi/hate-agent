<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import FileNode from './FileNode.svelte';
  import type { SymbolHit } from '../types';

  // Left pane: symbol search box + file tree.
  let query = $state('');

  const KIND_BADGE: Record<SymbolHit['kind'], string> = {
    def: 'def',
    ref: 'ref',
    call: 'call',
  };

  const results = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [] as SymbolHit[];
    return session.symbols.filter((s) => s.name.toLowerCase().includes(q)).slice(0, 12);
  });

  function pick(hit: SymbolHit): void {
    session.selectedPath = hit.path;
    query = '';
  }
</script>

<div class="left-pane">
  <div class="search-wrap">
    <Icon name="search" size={12} />
    <input
      class="search"
      placeholder="Search symbols…"
      bind:value={query}
      aria-label="Search symbols"
      spellcheck="false"
    />
    {#if query}
      <button class="clear" onclick={() => (query = '')} aria-label="Clear search">
        <Icon name="x" size={11} />
      </button>
    {/if}
  </div>

  {#if results.length > 0}
    <ul class="results" role="listbox" aria-label="Symbol results">
      {#each results as hit}
        <li>
          <button class="hit" onclick={() => pick(hit)}>
            <span class="kind">{KIND_BADGE[hit.kind]}</span>
            <span class="sname">{hit.name}</span>
            <span class="swhere">{hit.path}:{hit.line}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <nav class="tree" aria-label="Files">
    {#each session.tree as node (node.path)}
      <FileNode node={node} />
    {/each}
  </nav>
</div>

<style>
  .left-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .search-wrap {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px;
    padding: 0 8px;
    height: 26px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--fg-2);
    flex: none;
  }
  .search {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    outline: none;
    font-size: 12px;
    color: var(--fg-0);
  }
  .search::placeholder {
    color: var(--fg-3);
  }
  .clear {
    display: inline-flex;
    color: var(--fg-3);
  }
  .clear:hover {
    color: var(--fg-0);
  }
  .results {
    list-style: none;
    margin: 0 8px 6px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-2);
    flex: none;
    max-height: 220px;
    overflow: auto;
  }
  .hit {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 24px;
    padding: 0 8px;
    text-align: left;
    color: var(--fg-1);
  }
  .hit:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .kind {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--accent);
    width: 30px;
    flex: none;
  }
  .sname {
    font-family: var(--font-mono);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .swhere {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 45%;
  }
  .tree {
    flex: 1;
    overflow: auto;
    padding: 2px 4px 12px;
  }
</style>
