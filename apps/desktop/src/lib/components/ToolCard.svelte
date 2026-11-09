<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type { TranscriptBlock } from '../types';

  // One-line collapsible tool card. Header shows the essentials
  // (`edit src/x.rs +12 -4`); expanding reveals the shaped preview and the
  // full-vs-sent token savings.
  interface Props {
    block: TranscriptBlock;
  }

  let { block }: Props = $props();

  const TOOL_ICON: Record<string, string> = {
    read: 'file',
    read_file: 'file',
    list_dir: 'folder',
    glob: 'search',
    edit: 'edit',
    write: 'edit',
    bash: 'terminal',
    grep: 'search',
    find: 'search',
    symbol: 'search',
    apply_patch: 'diff',
    git_diff: 'diff',
    git_log: 'branch',
  };

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
  const statusColor = $derived(
    block.ok === undefined ? 'var(--fg-3)' : block.ok ? 'var(--ok)' : 'var(--err)',
  );

  const fmtTok = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`);

  const savings = $derived(
    block.fullTokens !== undefined && block.sentTokens !== undefined
      ? `${fmtTok(block.fullTokens)} → ${fmtTok(block.sentTokens)}`
      : null,
  );
</script>

<div class="tool-card rise" class:expanded={block.expanded}>
  <button
    class="head"
    onclick={() => (block.expanded = !block.expanded)}
    aria-expanded={block.expanded}
  >
    <span class="icon"><Icon name={TOOL_ICON[block.call?.name ?? ''] ?? 'terminal'} size={12} /></span>
    <span class="name">{block.call?.name ?? 'tool'}</span>
    <span class="summary mono">{summary}</span>
    {#if savings}
      <span class="savings mono" title="full tokens → sent tokens">{savings} tok</span>
    {/if}
    <span class="status" style:color={statusColor}>
      <Icon name={statusIcon} size={12} />
    </span>
  </button>
  {#if block.expanded}
    <div class="body">
      {#if block.preview}
        <pre class="preview mono">{block.preview}</pre>
      {:else}
        <pre class="preview mono dim">{JSON.stringify(block.call?.args ?? {}, null, 2)}</pre>
      {/if}
    </div>
  {/if}
</div>

<style>
  .tool-card {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-1);
    overflow: hidden;
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
  }
  .head:hover {
    background: var(--bg-2);
  }
  .icon {
    color: var(--fg-2);
    display: inline-flex;
    flex: none;
  }
  .name {
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-weight: 600;
    color: var(--fg-0);
    flex: none;
  }
  .summary {
    flex: 1;
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
</style>
