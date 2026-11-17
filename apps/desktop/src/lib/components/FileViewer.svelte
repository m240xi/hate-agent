<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import { copyText } from '../format';
  import { highlightLine, languageForPath } from '../highlight';

  let codeEl = $state<HTMLDivElement | null>(null);
  let copied = $state(false);
  let diffLayout = $state<'unified' | 'split'>('unified');
  let editing = $state(false);
  let editText = $state('');
  let editBusy = $state(false);

  const viewer = $derived(session.viewer);
  const view = $derived(viewer?.kind === 'file' ? viewer.view : null);
  const diff = $derived(viewer?.kind === 'diff' ? viewer.diff : null);
  const name = $derived(viewer ? (viewer.path.split(/[\\/]/).pop() ?? viewer.path) : '');
  const pinned = $derived(viewer ? session.isPinned(viewer.path) : false);
  const lang = $derived(viewer ? languageForPath(viewer.path) : 'plain');

  interface HunkLine {
    text: string;
    kind: 'add' | 'del' | 'meta' | 'ctx';
  }

  interface DiffHunk {
    header: string;
    patch: string;
    lines: HunkLine[];
  }

  interface SplitRow {
    left: { num: string; text: string; kind: 'del' | 'ctx' | 'empty' };
    right: { num: string; text: string; kind: 'add' | 'ctx' | 'empty' };
  }

  const parsedDiff = $derived.by(() => {
    if (!diff) return { fileHeader: [] as string[], hunks: [] as DiffHunk[] };
    const rawLines = diff.unified.split('\n');
    if (rawLines.at(-1) === '') rawLines.pop();
    const fileHeader: string[] = [];
    const hunks: DiffHunk[] = [];
    let currentHeader = '';
    let currentLines: string[] = [];

    const flushHunk = () => {
      if (!currentHeader) return;
      const patch = [...fileHeader, currentHeader, ...currentLines].join('\n') + '\n';
      hunks.push({
        header: currentHeader,
        patch,
        lines: currentLines.map((text) => {
          if (text.startsWith('+')) return { text, kind: 'add' };
          if (text.startsWith('-')) return { text, kind: 'del' };
          if (text.startsWith('\\')) return { text, kind: 'meta' };
          return { text, kind: 'ctx' };
        }),
      });
      currentHeader = '';
      currentLines = [];
    };

    for (const line of rawLines) {
      if (line.startsWith('@@')) {
        flushHunk();
        currentHeader = line;
      } else if (currentHeader) {
        currentLines.push(line);
      } else {
        fileHeader.push(line);
      }
    }
    flushHunk();
    return { fileHeader, hunks };
  });

  function buildSplitRows(hunk: DiffHunk): SplitRow[] {
    const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(hunk.header);
    let oldLine = m ? Number(m[1]) : 1;
    let newLine = m ? Number(m[2]) : 1;
    const rows: SplitRow[] = [];
    const dels: { num: string; text: string }[] = [];
    const adds: { num: string; text: string }[] = [];

    const flushPairs = () => {
      const n = Math.max(dels.length, adds.length);
      for (let i = 0; i < n; i++) {
        const d = dels[i];
        const a = adds[i];
        rows.push({
          left: d ? { num: d.num, text: d.text, kind: 'del' } : { num: '', text: '', kind: 'empty' },
          right: a ? { num: a.num, text: a.text, kind: 'add' } : { num: '', text: '', kind: 'empty' },
        });
      }
      dels.length = 0;
      adds.length = 0;
    };

    for (const l of hunk.lines) {
      if (l.kind === 'del') {
        dels.push({ num: String(oldLine++), text: l.text.slice(1) });
      } else if (l.kind === 'add') {
        adds.push({ num: String(newLine++), text: l.text.slice(1) });
      } else {
        flushPairs();
        const body = l.text.startsWith(' ') ? l.text.slice(1) : l.text;
        rows.push({
          left: { num: String(oldLine++), text: body, kind: 'ctx' },
          right: { num: String(newLine++), text: body, kind: 'ctx' },
        });
      }
    }
    flushPairs();
    return rows;
  }

  $effect(() => {
    void viewer?.path;
    void viewer?.kind;
    editing = false;
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

  function askAiSelection(): void {
    if (!viewer) return;
    const range = selectedRange();
    session.askAiAboutLines(viewer.path, range?.start ?? null, range?.end ?? null);
  }

  async function enterEdit(): Promise<void> {
    if (!viewer || !view || view.binary || view.too_large) return;
    editBusy = true;
    try {
      if (view.start > 1 || view.truncated) {
        const m = await import('../ipc');
        const full = await m.workspaceRead(viewer.path, 1, 0);
        if (full.too_large || full.binary) {
          session.note('file is too large or binary to edit inline');
          return;
        }
        editText = full.lines.join('\n');
      } else {
        editText = view.lines.join('\n');
      }
      editing = true;
    } catch (e) {
      session.note(`couldn't load full file: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      editBusy = false;
    }
  }

  async function saveEdit(): Promise<void> {
    if (editBusy) return;
    editBusy = true;
    const ok = await session.saveViewerFile(editText);
    editBusy = false;
    if (ok) editing = false;
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
      {#if session.changedFiles.length > 1 && session.reviewIndex >= 0}
        <span class="v-nav mono">
          <button class="v-btn icon" onclick={() => session.prevChangedFile()} title="Previous changed file" aria-label="Previous changed file">
            <Icon name="chevron-left" size={11} />
          </button>
          <span>{session.reviewIndex + 1}/{session.changedFiles.length}</span>
          <button class="v-btn icon" onclick={() => session.nextChangedFile()} title="Next changed file" aria-label="Next changed file">
            <Icon name="chevron-right" size={11} />
          </button>
        </span>
      {/if}
    {/if}
    <span class="spacer"></span>
    {#if viewer}
      {#if viewer.kind === 'file' && view && !view.binary && !view.too_large}
        {#if editing}
          <button class="v-btn on" onclick={() => void saveEdit()} disabled={editBusy} title="Save file (Ctrl+S)">
            <Icon name="save" size={12} />
            {editBusy ? 'saving…' : 'save'}
          </button>
          <button class="v-btn" onclick={() => (editing = false)} disabled={editBusy}>cancel</button>
        {:else}
          <button class="v-btn" onclick={() => void enterEdit()} disabled={editBusy} title="Edit file inline">
            <Icon name="edit" size={12} />
            edit
          </button>
          <button class="v-btn" onclick={askAiSelection} title="Ask AI about selected lines or file">
            <Icon name="sparkles" size={12} />
            ask AI
          </button>
        {/if}
      {/if}
      {#if viewer.kind === 'diff' && diff}
        <button
          class="v-btn"
          class:on={diffLayout === 'split'}
          onclick={() => (diffLayout = diffLayout === 'unified' ? 'split' : 'unified')}
          title={diffLayout === 'unified' ? 'Switch to side-by-side diff' : 'Switch to unified diff'}
        >
          <Icon name="columns" size={12} />
          {diffLayout === 'unified' ? 'split' : 'unified'}
        </button>
        <button
          class="v-btn danger"
          onclick={() => void session.revertChangedFile(viewer.path)}
          title="Discard all uncommitted changes in this file"
        >
          <Icon name="undo" size={12} />
          revert file
        </button>
      {/if}
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
      {:else if editing}
        <textarea
          class="inline-editor mono"
          bind:value={editText}
          spellcheck="false"
          onkeydown={(e) => {
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
              e.preventDefault();
              void saveEdit();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              editing = false;
            }
          }}
        ></textarea>
      {:else if view.lines.length === 0}
        <p class="note">empty file.</p>
      {:else}
        <div class="code" bind:this={codeEl}>
          {#each view.lines as text, i (view.start + i)}
            {@const n = view.start + i}
            {@const tokens = highlightLine(text, lang)}
            <div class="row" class:target={viewer?.line === n} data-line={n}>
              <span class="num">{n}</span>
              <span class="text">
                {#each tokens as tok}<span class={tok.cls ?? ''}>{tok.text}</span>{/each}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    {:else if diff}
      {#if parsedDiff.hunks.length === 0}
        <p class="note">no changes against HEAD.</p>
      {:else}
        <div class="code diff">
          {#each parsedDiff.hunks as hunk, hi (hi)}
            <div class="hunk-head mono">
              <span class="hunk-range">{hunk.header}</span>
              {#if diff.tracked}
                <button
                  class="v-btn"
                  onclick={() => void session.revertDiffHunk(viewer!.path, hunk.patch)}
                  title="Revert only this hunk"
                >
                  <Icon name="undo" size={11} /> revert hunk
                </button>
              {/if}
            </div>
            {#if diffLayout === 'split'}
              <div class="split-grid">
                {#each buildSplitRows(hunk) as sr}
                  <div class="srow">
                    <span class="scell {sr.left.kind}">
                      <span class="snum">{sr.left.num}</span>
                      <span class="stext">{sr.left.text}</span>
                    </span>
                    <span class="scell {sr.right.kind}">
                      <span class="snum">{sr.right.num}</span>
                      <span class="stext">{sr.right.text}</span>
                    </span>
                  </div>
                {/each}
              </div>
            {:else}
              {#each hunk.lines as l}
                <div class="drow {l.kind}"><span class="dtext">{l.text}</span></div>
              {/each}
            {/if}
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  <footer class="view-foot">
    {#if view && view.truncated && !editing}
      <button class="v-btn" onclick={() => session.loadMoreLines()}>load more</button>
    {/if}
    {#if editing}
      <span class="hint">editing inline · <b>Ctrl+S</b> save · <b>Esc</b> cancel</span>
    {:else if view && !view.binary && !view.too_large}
      <span class="hint">select lines, then <b>attach</b> or <b>ask AI</b></span>
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
    gap: 6px;
    min-height: 32px;
    padding: 3px 8px;
    border-bottom: 1px solid var(--border);
    flex: none;
    color: var(--fg-2);
    flex-wrap: wrap;
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
  .v-nav {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 10.5px;
    color: var(--fg-2);
  }
  .v-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 7px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-1);
    flex: none;
  }
  .v-btn.icon {
    padding: 2px 4px;
  }
  .v-btn:hover:not(:disabled) {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .v-btn.on {
    color: var(--accent);
    border-color: var(--accent-dim);
  }
  .v-btn.danger:hover:not(:disabled),
  .v-btn.close:hover:not(:disabled) {
    color: var(--err);
    border-color: var(--err);
  }
  .v-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .view-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .inline-editor {
    flex: 1;
    width: 100%;
    min-height: 100%;
    padding: 10px 14px;
    background: var(--bg-0);
    color: var(--fg-0);
    border: none;
    outline: none;
    resize: none;
    font-size: 12px;
    line-height: 1.55;
    tab-size: 2;
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
  .text :global(.hl-kw) {
    color: var(--accent);
    font-weight: 500;
  }
  .text :global(.hl-type) {
    color: var(--warn);
  }
  .text :global(.hl-str) {
    color: var(--ok);
  }
  .text :global(.hl-num) {
    color: color-mix(in srgb, var(--warn) 75%, var(--fg-0));
  }
  .text :global(.hl-com) {
    color: var(--fg-3);
    font-style: italic;
  }
  .diff {
    padding: 6px 0;
  }
  .hunk-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 10px;
    margin-top: 4px;
    background: var(--bg-1);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    color: var(--fg-3);
    font-size: 11px;
  }
  .hunk-range {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .split-grid {
    display: flex;
    flex-direction: column;
    font-size: 11px;
  }
  .srow {
    display: grid;
    grid-template-columns: 1fr 1fr;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 45%, transparent);
  }
  .scell {
    display: flex;
    gap: 8px;
    padding: 0 8px;
    min-width: 0;
    overflow: hidden;
  }
  .scell:first-child {
    border-right: 1px solid var(--border);
  }
  .scell.del {
    background: color-mix(in srgb, var(--err) 12%, transparent);
  }
  .scell.add {
    background: color-mix(in srgb, var(--ok) 12%, transparent);
  }
  .scell.empty {
    background: color-mix(in srgb, var(--bg-1) 50%, transparent);
  }
  .snum {
    width: 30px;
    flex: none;
    text-align: right;
    color: var(--fg-3);
    user-select: none;
  }
  .stext {
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--fg-1);
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
