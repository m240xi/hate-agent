<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import { trapFocus } from '../focus';
  import type { ContextRef, FileNode } from '../types';

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  interface PickerItem {
    key: string;
    label: string;
    hint: string;
    icon: string;
    kind: 'file' | 'folder' | 'context' | 'url_prompt';
    path?: string;
    context?: ContextRef;
  }

  function flatten(
    nodes: FileNode[],
    prefix = '',
    files: string[] = [],
    dirs: string[] = [],
  ): { files: string[]; dirs: string[] } {
    for (const n of nodes) {
      const p = prefix ? `${prefix}/${n.name}` : n.name;
      if (n.kind === 'file') files.push(p);
      else if (n.kind === 'dir') dirs.push(p);
      if (n.children) flatten(n.children, p, files, dirs);
    }
    return { files, dirs };
  }

  const flattened = $derived(flatten(session.tree));
  let query = $state('');
  let sel = $state(0);
  let filterEl = $state<HTMLInputElement | null>(null);
  let urlMode = $state(false);
  let urlInput = $state('');
  let urlEl = $state<HTMLInputElement | null>(null);

  onMount(() => filterEl?.focus());

  const results = $derived.by((): PickerItem[] => {
    const raw = query.trim();
    const q = raw.toLowerCase();
    const items: PickerItem[] = [];

    if (/^https?:\/\//i.test(raw)) {
      items.push({
        key: `url:${raw}`,
        label: `@url ${raw}`,
        hint: 'fetch & attach web page text',
        icon: 'globe',
        kind: 'context',
        context: { kind: 'url', url: raw },
      });
    } else {
      items.push({
        key: 'ctx:url',
        label: '@url <link>',
        hint: 'fetch & attach a web page',
        icon: 'globe',
        kind: 'url_prompt',
      });
    }

    if (session.terminals.length > 0) {
      const active = session.terminals.find((t) => t.id === session.activeTerminalId) ?? session.terminals[0];
      items.push({
        key: 'ctx:terminal',
        label: '@terminal',
        hint: `attach output from ${active?.title ?? 'terminal'}`,
        icon: 'terminal',
        kind: 'context',
        context: { kind: 'terminal', id: active?.id ?? null, lines: 120 },
      });
    }

    if (session.workspacePath) {
      items.push(
        {
          key: 'ctx:git_diff',
          label: '@git:diff',
          hint: 'working tree changes (git diff HEAD)',
          icon: 'diff',
          kind: 'context',
          context: { kind: 'git_diff' },
        },
        {
          key: 'ctx:git_staged',
          label: '@git:staged',
          hint: 'staged changes (git diff --cached)',
          icon: 'diff',
          kind: 'context',
          context: { kind: 'git_staged' },
        },
      );
      for (const d of flattened.dirs) {
        items.push({
          key: `dir:${d}`,
          label: `@folder:${d}`,
          hint: 'folder summary',
          icon: 'folder',
          kind: 'folder',
          context: { kind: 'folder', path: d },
        });
      }
      for (const f of flattened.files) {
        items.push({
          key: `file:${f}`,
          label: `@${f}`,
          hint: 'file',
          icon: 'file',
          kind: 'file',
          path: f,
        });
      }
    }

    if (!q) return items.slice(0, 18);
    return items
      .filter((it) => it.label.toLowerCase().includes(q) || it.hint.toLowerCase().includes(q))
      .slice(0, 18);
  });

  $effect(() => {
    void query;
    sel = 0;
  });

  function pick(item: PickerItem): void {
    if (item.kind === 'url_prompt') {
      urlMode = true;
      setTimeout(() => urlEl?.focus(), 0);
      return;
    }
    if (item.kind === 'file' && item.path) {
      session.draft = `${session.draft}@${item.path} `;
      onclose();
      return;
    }
    if (item.context) {
      session.addContext(item.context);
      onclose();
    }
  }

  function submitUrl(): void {
    const raw = urlInput.trim();
    if (!raw) return;
    const normalized = /^https?:\/\//i.test(raw) ? raw : `https://${raw}`;
    session.addContext({ kind: 'url', url: normalized });
    onclose();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (urlMode) {
        urlMode = false;
        setTimeout(() => filterEl?.focus(), 0);
      } else {
        onclose();
      }
      return;
    }
    if (urlMode) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      sel = Math.min(sel + 1, Math.max(0, results.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      sel = Math.max(sel - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const item = results[sel];
      if (item) pick(item);
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
  <div class="picker rise" role="dialog" aria-label="Attach context" tabindex="-1" use:trapFocus>
    {#if urlMode}
      <form
        class="filter-wrap"
        onsubmit={(e) => {
          e.preventDefault();
          submitUrl();
        }}
      >
        <Icon name="globe" size={13} />
        <input
          class="filter"
          placeholder="https://example.com/docs…"
          bind:value={urlInput}
          aria-label="Enter URL to attach"
          bind:this={urlEl}
        />
        <button class="btn primary sm" type="submit">Attach</button>
      </form>
    {:else}
      <div class="filter-wrap">
        <Icon name="paperclip" size={13} />
        <input
          class="filter"
          placeholder={session.workspacePath
            ? 'File, @git:diff, @terminal, @url, @folder…'
            : '@terminal, @url, or paste https://…'}
          bind:value={query}
          onkeydown={onKeydown}
          aria-label="Filter context references"
          bind:this={filterEl}
        />
      </div>
      <ul class="list" role="listbox">
        {#each results as item, i (item.key)}
          <li>
            <button
              class="path"
              class:sel={i === sel}
              onclick={() => pick(item)}
              onmouseenter={() => (sel = i)}
              role="option"
              aria-selected={i === sel}
            >
              <Icon name={item.icon} size={12} />
              <span class="p-lbl mono">{item.label}</span>
              <span class="p-hint">{item.hint}</span>
            </button>
          </li>
        {/each}
        {#if results.length === 0}
          <li class="none">no matching context</li>
        {/if}
      </ul>
    {/if}
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
    width: 490px;
    max-width: 90vw;
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
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 320px;
    overflow: auto;
  }
  .path {
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
  .path.sel {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .p-lbl {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .p-hint {
    margin-left: auto;
    font-size: 10.5px;
    color: var(--fg-3);
    flex: none;
  }
  .none {
    padding: 12px;
    text-align: center;
    color: var(--fg-3);
    font-size: 12px;
  }
</style>
