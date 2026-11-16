<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import { usageLabel, usageTitle } from '../format';
  import type { TranscriptBlock } from '../types';

  interface Props {
    block: TranscriptBlock;
    /** position inside a group: drives the cascade on insert */
    index?: number;
  }

  let { block, index = 0 }: Props = $props();

  type Tone = 'read' | 'write' | 'exec' | 'net';

  // glyph + tone per tool family: reads are safe, writes are the palette accent,
  // shell is caution, network is neutral. Identity without inventing hues.
  const TOOLS: Record<string, { icon: string; tone: Tone }> = {
    read: { icon: 'file', tone: 'read' },
    read_file: { icon: 'file', tone: 'read' },
    read_image: { icon: 'image', tone: 'read' },
    list_dir: { icon: 'folder', tone: 'read' },
    glob: { icon: 'search', tone: 'read' },
    edit: { icon: 'edit', tone: 'write' },
    edit_file: { icon: 'edit', tone: 'write' },
    multi_edit: { icon: 'edit', tone: 'write' },
    str_replace: { icon: 'edit', tone: 'write' },
    write: { icon: 'file-pen', tone: 'write' },
    write_file: { icon: 'file-pen', tone: 'write' },
    apply_patch: { icon: 'diff', tone: 'write' },
    bash: { icon: 'terminal', tone: 'exec' },
    shell: { icon: 'terminal', tone: 'exec' },
    run: { icon: 'terminal', tone: 'exec' },
    grep: { icon: 'search', tone: 'exec' },
    find: { icon: 'search', tone: 'exec' },
    symbol: { icon: 'search', tone: 'exec' },
    git_diff: { icon: 'diff', tone: 'exec' },
    git_log: { icon: 'branch', tone: 'exec' },
    web_search: { icon: 'sparkles', tone: 'net' },
    web_fetch: { icon: 'link', tone: 'net' },
    fetch: { icon: 'link', tone: 'net' },
  };

  const tool = $derived.by(() => {
    const name = block.call?.name ?? '';
    return (
      TOOLS[name] ??
      (name.startsWith('mcp__')
        ? { icon: 'plug', tone: 'net' as Tone }
        : { icon: 'terminal', tone: 'exec' as Tone })
    );
  });

  const toolState = $derived(block.ok === undefined ? 'run' : block.ok ? 'ok' : 'fail');

  const summary = $derived.by(() => {
    const call = block.call;
    if (!call) return '';
    const args = call.args as Record<string, unknown>;
    const path = typeof args?.path === 'string' ? args.path : typeof args?.query === 'string' ? args.query : '';
    const cmd = typeof args?.command === 'string' ? args.command : '';
    const target = path || cmd || JSON.stringify(args ?? {}).slice(0, 60);
    return target;
  });

  const statusIcon = $derived(
    block.ok === undefined ? 'clock' : block.ok ? 'check' : 'alert',
  );

  const linkPath = $derived.by(() => {
    const args = block.call?.args as Record<string, unknown> | undefined;
    const p = typeof args?.path === 'string' ? args.path : typeof args?.file === 'string' ? args.file : '';
    if (!p || p.startsWith('/') || /^[a-zA-Z]:[\\/]/.test(p) || p.includes('..')) return '';
    return p;
  });

  function toggle(): void {
    block.expanded = !block.expanded;
  }

  function onHeadKey(e: KeyboardEvent): void {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      toggle();
    }
  }
  const statusColor = $derived(
    block.ok === undefined ? 'var(--fg-3)' : block.ok ? 'var(--ok)' : 'var(--err)',
  );

  const fmtTok = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`);

  const WRITE_TOOLS = new Set(['edit', 'write', 'apply_patch', 'multi_edit', 'str_replace']);
  const isWriter = $derived(WRITE_TOOLS.has(block.call?.name ?? ''));

  let diff = $state<{ unified: string; added: number; removed: number } | null>(null);
  let diffError = $state<string | null>(null);
  let diffFor = $state<string | null>(null);

  $effect(() => {
    const path = linkPath;
    if (!block.expanded || !isWriter || !path || block.ok === false) return;
    if (diffFor === path) return;
    diffFor = path;
    void import('../ipc')
      .then((m) => m.workspaceDiff(path))
      .then((d) => {
        diff = d;
      })
      .catch((e: unknown) => {
        diffError = e instanceof Error ? e.message : String(e);
      });
  });

  const diffLines = $derived.by(() => {
    if (!diff?.unified) return [];
    return diff.unified
      .split('\n')
      .filter((l) => !l.startsWith('diff ') && !l.startsWith('index '))
      .slice(0, 80)
      .map((text) => ({
        text,
        kind: text.startsWith('@@')
          ? ('meta' as const)
          : text.startsWith('+++') || text.startsWith('---')
            ? ('meta' as const)
            : text.startsWith('+')
              ? ('add' as const)
              : text.startsWith('-')
                ? ('del' as const)
                : ('ctx' as const),
      }));
  });

  const savings = $derived(
    block.fullTokens !== undefined && block.sentTokens !== undefined
      ? `${fmtTok(block.fullTokens)} → ${fmtTok(block.sentTokens)}`
      : null,
  );
</script>

<div
  class="tool-card rise"
  class:expanded={block.expanded}
  data-state={toolState}
  data-ctx="tool"
  data-name={block.call?.name ?? ''}
  data-copy={summary}
  data-path={linkPath}
  data-writer={isWriter}
  style:--i={index}
>
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="head"
    role="button"
    tabindex="0"
    onclick={toggle}
    onkeydown={onHeadKey}
    aria-expanded={block.expanded}
  >
    <span class="icon" data-tone={tool.tone}><Icon name={tool.icon} size={11} /></span>
    <span class="name" title={block.call?.name ?? ''}>{block.call?.name ?? 'tool'}</span>
    <span class="summary mono">{summary}</span>
    {#if diff && block.ok !== false}
      <span class="fp mono" title="lines added and removed by this edit">
        <span class="add">+{diff.added}</span><span class="del">−{diff.removed}</span>
      </span>
    {:else if savings}
      <span class="savings mono" title="full tokens → sent tokens">{savings} tok</span>
    {/if}
    {#if linkPath}
      <button
        class="open-file"
        onclick={(e) => {
          e.stopPropagation();
          void session.openFile(linkPath);
        }}
        title={`Open ${linkPath}`}
        aria-label={`Open ${linkPath}`}
      >
        <Icon name="file" size={11} />
      </button>
    {/if}
    <span class="status" style:color={statusColor}>
      <Icon name={statusIcon} size={12} />
    </span>
  </div>
  {#if block.expanded}
    <div class="body">
      {#if block.usage}
        <div class="req-usage mono" title={usageTitle(block.usage)}>
          request {usageLabel(block.usage)}
        </div>
      {/if}
      {#if isWriter && diffLines.length > 0}
        <div class="diff">
          <div class="diff-head mono">
            <span class="add">+{diff?.added ?? 0}</span>
            <span class="del">-{diff?.removed ?? 0}</span>
            <span class="spacer"></span>
            <button
              class="diff-open"
              onclick={() => void session.openDiff(linkPath)}
              title="Open the full diff"
            >
              full diff
            </button>
          </div>
          <pre class="diff-body mono"
            >{#each diffLines as l}<span class={l.kind}>{l.text}</span>{/each}</pre>
        </div>
      {:else if isWriter && diffError}
        <p class="diff-note mono">{diffError}</p>
      {/if}
      {#if block.preview}
        <pre class="preview mono">{block.preview}</pre>
      {:else}
        <pre class="preview mono dim">{JSON.stringify(block.call?.args ?? {}, null, 2)}</pre>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* A burst of calls cascades instead of popping in one frame. Capped so a
     long group never takes more than ~a fifth of a second to settle. */
  .tool-card.rise {
    animation-delay: calc(min(var(--i, 0), 10) * 22ms);
  }
  .tool-card {
    position: relative;
    flex: none;
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-1);
    overflow: hidden;
    box-shadow: var(--el-1);
    /* outcome rail: the fastest way to scan a long transcript */
    --rail: var(--border);
  }
  .tool-card[data-state='run'] {
    --rail: color-mix(in srgb, var(--accent) 55%, var(--border));
  }
  .tool-card[data-state='ok'] {
    --rail: color-mix(in srgb, var(--ok) 65%, var(--border));
  }
  .tool-card[data-state='fail'] {
    --rail: color-mix(in srgb, var(--err) 80%, var(--border));
  }
  .tool-card::before {
    content: '';
    position: absolute;
    inset: 0 auto 0 0;
    width: 2px;
    background: var(--rail);
    pointer-events: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 26px;
    padding: 0 8px;
    text-align: left;
    color: var(--fg-1);
    cursor: pointer;
  }
  .head:hover {
    background: var(--bg-2);
  }
  .diff {
    margin: 0 0 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-0);
  }
  .diff-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    font-size: 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-1);
  }
  .diff-head .add {
    color: var(--ok);
  }
  .diff-head .del {
    color: var(--err);
  }
  .diff-head .spacer {
    flex: 1 1 auto;
  }
  .diff-open {
    font-size: 10px;
    color: var(--fg-3);
  }
  .diff-open:hover {
    color: var(--fg-0);
    text-decoration: underline;
  }
  .diff-body {
    margin: 0;
    padding: 6px 8px;
    max-height: 220px;
    overflow: auto;
    font-size: 10.5px;
    line-height: 1.5;
  }
  /* git's rails: a tinted band plus a 2px rule, so a diff reads by shape
     before a single line is read. */
  .diff-body .add {
    color: var(--ok);
    display: block;
    background: color-mix(in srgb, var(--ok) 9%, transparent);
    box-shadow: inset 2px 0 0 color-mix(in srgb, var(--ok) 65%, transparent);
  }
  .diff-body .del {
    color: var(--err);
    display: block;
    background: color-mix(in srgb, var(--err) 9%, transparent);
    box-shadow: inset 2px 0 0 color-mix(in srgb, var(--err) 70%, transparent);
  }
  .diff-body .meta {
    color: var(--info);
    display: block;
  }
  .diff-body .ctx {
    color: var(--fg-3);
    display: block;
  }
  .diff-note {
    margin: 0 0 8px;
    font-size: 10px;
    color: var(--fg-3);
  }
  .open-file {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: var(--radius);
    color: var(--fg-3);
    flex: none;
    opacity: 0;
  }
  .head:hover .open-file,
  .open-file:focus-visible {
    opacity: 1;
  }
  .open-file:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 18px;
    height: 18px;
    border-radius: 5px;
    color: var(--fg-2);
    background: color-mix(in srgb, var(--fg-2) 10%, transparent);
  }
  .icon[data-tone='read'] {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 14%, transparent);
  }
  .icon[data-tone='write'] {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }
  .icon[data-tone='exec'] {
    color: var(--warn);
    background: color-mix(in srgb, var(--warn) 14%, transparent);
  }
  .icon[data-tone='net'] {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .fp {
    display: inline-flex;
    gap: 5px;
    font-size: 10px;
    flex: none;
  }
  .fp .add {
    color: var(--ok);
  }
  .fp .del {
    color: var(--err);
  }
  .name {
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-weight: 600;
    color: var(--fg-0);
    flex: 0 1 auto;
    min-width: 40px;
    max-width: 60%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .summary {
    flex: 1 1 0;
    min-width: 0;
    font-size: 11px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .savings {
    font-size: 10px;
    color: var(--fg-3);
    flex: none;
  }
  .status {
    display: inline-flex;
    flex: none;
  }
  .body {
    border-top: 1px solid var(--border);
    padding: 8px;
    background: var(--bg-0);
  }
  .preview {
    margin: 0;
    font-size: 11px;
    line-height: 1.5;
    color: var(--fg-1);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 320px;
    overflow: auto;
  }
  .dim {
    color: var(--fg-3);
  }
  .req-usage {
    font-size: 10px;
    color: var(--fg-3);
    text-align: right;
    margin-bottom: 6px;
  }
</style>
