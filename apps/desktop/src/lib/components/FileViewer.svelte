<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import { copyText } from '../format';

  let codeEl = $state<HTMLDivElement | null>(null);
  let copied = $state(false);

  const viewer = $derived(session.viewer);
  const view = $derived(viewer?.kind === 'file' ? viewer.view : null);
  const diff = $derived(viewer?.kind === 'diff' ? viewer.diff : null);
  const name = $derived(viewer ? (viewer.path.split(/[\\/]/).pop() ?? viewer.path) : '');
  const pinned = $derived(viewer ? session.isPinned(viewer.path) : false);

  const diffLines = $derived.by(() => {
    if (!diff) return [] as { text: string; kind: 'add' | 'del' | 'meta' | 'ctx' }[];
    return diff.unified
      .split('\n')
      .filter((l, i, all) => !(i === all.length - 1 && l === ''))
      .map((text) => {
        if (text.startsWith('@@')) return { text, kind: 'meta' as const };
        if (text.startsWith('+++') || text.startsWith('---') || text.startsWith('diff '))
          return { text, kind: 'meta' as const };
        if (text.startsWith('+')) return { text, kind: 'add' as const };
        if (text.startsWith('-')) return { text, kind: 'del' as const };
        return { text, kind: 'ctx' as const };
      });
  });

  $effect(() => {
    const line = viewer?.line;
    if (!line || !codeEl) return;
    const row = codeEl.querySelector(`[data-line="${line}"]`);
    row?.scrollIntoView({ block: 'center' });
  });

  function selectedRange(): { start: number; end: number } | null {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
    const range = sel.getRangeAt(0);
    const pick = (node: Node | null): number | null => {
      const el = (node?.nodeType === 1 ? node : node?.parentElement) as HTMLElement | null;
      const row = el?.closest('[data-line]') as HTMLElement | null;
      const n = Number(row?.dataset.line);
      return Number.isFinite(n) ? n : null;
    };
    const a = pick(range.startContainer);
    const b = pick(range.endContainer);
    if (a === null || b === null) return null;
    return { start: Math.min(a, b), end: Math.max(a, b) };
  }

  function pinSelection(): void {
    if (!viewer) return;
    const range = selectedRange();
    if (range) session.pinFile(viewer.path, range.start, range.end);
    else session.pinFile(viewer.path);
  }

  async function copyPath(): Promise<void> {
    if (!viewer) return;
    copied = await copyText(viewer.path);
    setTimeout(() => (copied = false), 1200);
  }
</script>

<div class="viewer">
  <header class="view-head">
    <Icon name={viewer?.kind === 'diff' ? 'diff' : 'file'} size={13} />
    <span class="v-name" title={viewer?.path ?? ''}>{name}</span>
    {#if view}
      <span class="v-meta mono">
        {view.total_lines} lines
        {#if view.start > 1}· from {view.start}{/if}
      </span>
    {:else if diff}
      <span class="v-meta mono"><span class="add">+{diff.added}</span> <span class="del">-{diff.removed}</span></span>
    {/if}
    <span class="spacer"></span>
    {#if viewer}
      <button
        class="v-btn"
        class:on={pinned}
        onclick={() => (viewer.kind === 'file' ? pinSelection() : session.togglePin(viewer.path))}
        title={pinned ? 'Remove from next message' : 'Attach to next message'}
      >
        <Icon name={pinned ? 'check' : 'plus'} size={12} />
        {pinned ? 'attached' : 'attach'}
      </button>
      <button class="v-btn" onclick={copyPath} title="Copy relative path">
        {copied ? 'copied' : 'copy path'}
      </button>
      <button
        class="v-btn"
        onclick={() => (viewer.kind === 'diff' ? session.openFile(viewer.path) : session.openDiff(viewer.path))}
        title={viewer.kind === 'diff' ? 'Show the file' : 'Show changes'}
      >
        <Icon name={viewer.kind === 'diff' ? 'file' : 'diff'} size={12} />
      </button>
      <button class="v-btn close" onclick={() => session.closeViewer()} aria-label="Close preview" title="Close (Esc)">
        <Icon name="x" size={12} />
      </button>
    {/if}
  </header>

  <div class="view-body">
    {#if !viewer}
      <p class="note">Nothing open.</p>
    {:else if viewer.loading}
      <p class="note">loading…</p>
    {:else if viewer.error}
      <p class="note err">{viewer.error}</p>
    {:else if view}
      {#if view.binary}
        <p class="note">binary file — not shown.</p>
      {:else if view.too_large}
        <p class="note">file is too large to preview (4 MB limit).</p>
      {:else if view.lines.length === 0}
        <p class="note">empty file.</p>
      {:else}
        <div class="code" bind:this={codeEl}>
          {#each view.lines as text, i (view.start + i)}
            {@const n = view.start + i}
            <div class="row" class:target={viewer?.line === n} data-line={n}>
              <span class="num">{n}</span>
              <span class="text">{text}</span>
            </div>
          {/each}
        </div>
      {/if}
    {:else if diff}
      {#if diffLines.length === 0}
        <p class="note">no changes against HEAD.</p>
      {:else}
        <div class="code diff">
          {#each diffLines as l}
            <div class="drow {l.kind}"><span class="dtext">{l.text}</span></div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  <footer class="view-foot">
    {#if view && view.truncated}
      <button class="v-btn" onclick={() => session.loadMoreLines()}>load more</button>
    {/if}
    {#if view && !view.binary && !view.too_large}
      <span class="hint">select lines, then <b>attach</b> to send just that range</span>
    {/if}
    <span class="spacer"></span>
    <span class="path mono">{viewer?.path ?? ''}</span>
  </footer>
</div>

<style>
  .viewer {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-0);
  }
  .view-head {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 8px;
    border-bottom: 1px solid var(--border);
    flex: none;
    color: var(--fg-2);
  }
  .v-name {
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-0);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .v-meta {
    font-size: 10.5px;
    color: var(--fg-3);
    flex: none;
  }
  .v-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 7px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-1);
    flex: none;
  }
  .v-btn:hover {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .v-btn.on {
    color: var(--accent);
    border-color: var(--accent-dim);
  }
  .v-btn.close:hover {
    color: var(--err);
    border-color: var(--err);
  }
  .view-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .code {
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.55;
    user-select: text;
    padding: 6px 0 24px;
  }
  .row {
    display: flex;
    gap: 10px;
    padding: 0 10px;
  }
  .row:hover {
    background: var(--bg-1);
  }
  .row.target {
    background: var(--bg-2);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .num {
    color: var(--fg-3);
    width: 40px;
    flex: none;
    text-align: right;
    user-select: none;
  }
  .text {
    white-space: pre;
    color: var(--fg-1);
    min-width: 0;
  }
  .diff {
    padding: 6px 0;
  }
  .drow {
    padding: 0 10px;
    white-space: pre;
    color: var(--fg-1);
  }
  .drow.add {
    background: color-mix(in srgb, var(--ok) 12%, transparent);
    color: var(--fg-0);
  }
  .drow.del {
    background: color-mix(in srgb, var(--err) 12%, transparent);
    color: var(--fg-0);
  }
  .drow.meta {
    color: var(--fg-3);
  }
  .note {
    margin: 16px;
    font-size: 12px;
    color: var(--fg-3);
  }
  .note.err {
    color: var(--err);
  }
  .view-foot {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 26px;
    padding: 0 10px;
    border-top: 1px solid var(--border);
    flex: none;
  }
  .hint {
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .hint b {
    color: var(--fg-2);
    font-weight: 600;
  }
  .path {
    font-size: 10px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 55%;
  }
  .spacer {
    flex: 1;
  }
  .add {
    color: var(--ok);
  }
  .del {
    color: var(--err);
  }
</style>
