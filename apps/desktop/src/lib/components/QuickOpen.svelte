<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import { trapFocus } from '../focus';
  import type { FileNode } from '../types';

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  interface QuickHit {
    kind: 'file' | 'symbol';
    label: string;
    detail: string;
    path: string;
    line: number | null;
    score: number;
  }

  function flattenFiles(nodes: FileNode[], prefix = '', out: string[] = []): string[] {
    for (const n of nodes) {
      const p = prefix ? `${prefix}/${n.name}` : n.name;
      if (n.kind === 'file') out.push(p);
      if (n.children) flattenFiles(n.children, p, out);
    }
    return out;
  }

  function fuzzyScore(text: string, q: string): number | null {
    if (!q) return 0;
    const hay = text.toLowerCase();
    let ti = 0;
    let total = 0;
    let run = 0;
    for (const ch of q) {
      const at = hay.indexOf(ch, ti);
      if (at < 0) return null;
      run = at === ti && ti > 0 ? run + 1 : 0;
      total += 1 + run * 2 - Math.min(at - ti, 8) * 0.15;
      ti = at + 1;
    }
    return total;
  }

  const allFiles = $derived(flattenFiles(session.tree));
  let query = $state('');
  let sel = $state(0);
  let inputEl = $state<HTMLInputElement | null>(null);

  onMount(() => inputEl?.focus());

  const results = $derived.by((): QuickHit[] => {
    const raw = query.trim();
    const symbolsOnly = raw.startsWith('@');
    const q = (symbolsOnly ? raw.slice(1) : raw).toLowerCase();

    if (!q && !symbolsOnly) {
      return allFiles.slice(0, 20).map((p) => ({
        kind: 'file',
        label: p.split(/[\\/]/).pop() ?? p,
        detail: p,
        path: p,
        line: null,
        score: 0,
      }));
    }

    const hits: QuickHit[] = [];
    if (!symbolsOnly) {
      for (const p of allFiles) {
        const s = fuzzyScore(p, q);
        if (s !== null) {
          hits.push({
            kind: 'file',
            label: p.split(/[\\/]/).pop() ?? p,
            detail: p,
            path: p,
            line: null,
            score: s + 0.5,
          });
        }
      }
    }
    if (q.length >= 2 || symbolsOnly) {
      for (const sym of session.symbols) {
        const s = fuzzyScore(sym.name, q);
        if (s !== null) {
          hits.push({
            kind: 'symbol',
            label: sym.name,
            detail: `${sym.path}:${sym.line}`,
            path: sym.path,
            line: sym.line,
            score: s,
          });
        }
      }
    }
    hits.sort((a, b) => b.score - a.score);
    return hits.slice(0, 24);
  });

  $effect(() => {
    void query;
    sel = 0;
  });

  function openHit(h: QuickHit): void {
    void session.openFile(h.path, h.line);
    onclose();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      sel = Math.min(sel + 1, Math.max(0, results.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      sel = Math.max(sel - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const h = results[sel];
      if (h) openHit(h);
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
  <div class="picker rise" role="dialog" aria-label="Quick open file or symbol" tabindex="-1" use:trapFocus>
    <div class="filter-wrap">
      <Icon name="search" size={13} />
      <input
        class="filter"
        placeholder="Open file or @symbol…"
        bind:value={query}
        onkeydown={onKeydown}
        aria-label="Search files and symbols"
        bind:this={inputEl}
      />
      <kbd class="esc-key mono">esc</kbd>
    </div>
    <ul class="list" role="listbox">
      {#each results as item, i (`${item.kind}:${item.detail}:${i}`)}
        <li>
          <button
            class="hit"
            class:sel={i === sel}
            onclick={() => openHit(item)}
            onmouseenter={() => (sel = i)}
            role="option"
            aria-selected={i === sel}
          >
            <Icon name={item.kind === 'symbol' ? 'sparkles' : 'file'} size={12} />
            <span class="h-label mono">{item.label}</span>
            <span class="h-detail mono">{item.detail}</span>
          </button>
        </li>
      {/each}
      {#if results.length === 0}
        <li class="none">
          {session.workspacePath ? 'no matching files or symbols' : 'attach a folder to search files'}
        </li>
      {/if}
    </ul>
    <div class="foot mono">
      <span>↑↓ move</span>
      <span>⏎ open</span>
      <span>@ symbol</span>
    </div>
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
    padding-top: 14vh;
    z-index: 50;
  }
  .picker {
    width: 520px;
    max-width: 92vw;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--el-3);
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
  .esc-key {
    font-size: 10px;
    padding: 1px 5px;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--fg-3);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 320px;
    overflow: auto;
  }
  .hit {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 28px;
    padding: 0 10px;
    border-radius: var(--radius);
    text-align: left;
    font-size: 12px;
    color: var(--fg-1);
  }
  .hit.sel {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .h-label {
    flex: none;
    max-width: 45%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-0);
  }
  .h-detail {
    flex: 1;
    min-width: 0;
    text-align: right;
    font-size: 10.5px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .none {
    padding: 14px;
    text-align: center;
    color: var(--fg-3);
    font-size: 12px;
  }
  .foot {
    display: flex;
    gap: 14px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    font-size: 10px;
    color: var(--fg-3);
  }
</style>
