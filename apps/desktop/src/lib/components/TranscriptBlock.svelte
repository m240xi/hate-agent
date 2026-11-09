<script lang="ts">
  import { session } from '../store.svelte';
  import ToolCard from './ToolCard.svelte';
  import { renderMarkdown } from '../markdown';
  import { copyText, fmtTok, turnLabel, usageLabel, usageTitle } from '../format';
  import type { TranscriptBlock } from '../types';

  interface Props {
    block: TranscriptBlock;
  }

  let { block }: Props = $props();

  let thinkingOpen = $state(false);
  let copied = $state(false);

  function flashCopied(): void {
    copied = true;
    setTimeout(() => (copied = false), 1200);
  }

  async function copyMessage(): Promise<void> {
    if (await copyText(block.text ?? '')) flashCopied();
  }

  async function onMessageClick(e: MouseEvent): Promise<void> {
    const btn = (e.target as HTMLElement | null)?.closest('.copy-code');
    if (!btn) return;
    const code = btn.parentElement?.querySelector('code')?.textContent ?? '';
    if (await copyText(code)) {
      btn.textContent = 'copied';
      setTimeout(() => (btn.textContent = 'copy'), 1200);
    }
  }
</script>

{#if block.kind === 'user'}
  <div class="user-msg rise">
    <div class="user-tag">you{#if block.plan}<span class="plan-tag">plan</span>{/if}</div>
    <div class="user-text">{block.text}</div>
    {#if block.turn}
      <div class="usage mono" title={`whole turn\n${usageTitle(block.turn.usage)}`}>
        {turnLabel(block.turn)}
      </div>
    {/if}
  </div>
{:else if block.kind === 'text'}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="assistant-msg" class:local={block.local}>
    <div class="assistant-text" class:streaming={block.streaming} onclick={onMessageClick}>
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html renderMarkdown(block.text ?? '')}
      {#if block.streaming}<span class="caret" aria-hidden="true"></span>{/if}
    </div>
    {#if !block.streaming && !block.local}
      <div class="msg-foot">
        <button class="copy-msg" onclick={copyMessage} aria-label="Copy message">
          {copied ? 'copied' : 'copy'}
        </button>
        {#if block.usage}
          <span class="usage mono" title={usageTitle(block.usage)}>{usageLabel(block.usage)}</span>
        {/if}
      </div>
    {/if}
  </div>
{:else if block.kind === 'thinking'}
  <div class="thinking">
    <button
      class="thinking-head"
      onclick={() => (thinkingOpen = !thinkingOpen)}
      aria-expanded={thinkingOpen}
    >
      <span class="chev">{thinkingOpen ? '▾' : '▸'}</span>
      thinking
      {#if block.streaming}<span class="live">…</span>{/if}
      {#if block.thinkingTokens}
        <span
          class="usage mono"
          title={block.thinkingEstimated
            ? 'reasoning tokens, counted locally — this provider bills thinking inside output'
            : 'reasoning tokens reported by the provider'}
        >
          {block.thinkingEstimated ? '~' : ''}{fmtTok(block.thinkingTokens)} tok
        </span>
      {/if}
      {#if block.usage}
        <span class="usage mono req" title={usageTitle(block.usage)}>
          {usageLabel(block.usage, false)}
        </span>
      {/if}
    </button>
    {#if thinkingOpen}
      <div class="thinking-body">{block.text}</div>
    {/if}
  </div>
{:else if block.kind === 'tool' && block.call}
  <ToolCard {block} />
{:else if block.kind === 'error'}
  <div class="error-msg rise">{block.text}</div>
{/if}

<style>
  .user-msg {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    align-self: flex-end;
    max-width: 85%;
  }
  .user-tag {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .user-text {
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--fg-0);
  }
  .plan-tag {
    margin-left: 6px;
    padding: 0 4px;
    border: 1px solid var(--accent-dim);
    border-radius: 3px;
    color: var(--accent);
  }
  .usage {
    font-size: 10px;
    color: var(--fg-3);
    white-space: nowrap;
  }
  .user-msg .usage {
    align-self: flex-end;
    margin-top: 2px;
    white-space: normal;
    text-align: right;
  }
  .assistant-msg {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .assistant-msg.local .assistant-text {
    color: var(--fg-2);
    font-size: 12px;
  }
  .msg-foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 2px 10px;
    min-height: 14px;
  }
  .copy-msg {
    font-size: 10px;
    color: var(--fg-3);
    opacity: 0;
    transition: opacity var(--motion);
  }
  .assistant-msg:hover .copy-msg,
  .copy-msg:focus-visible {
    opacity: 1;
  }
  .copy-msg:hover {
    color: var(--fg-0);
  }
  .assistant-text {
    color: var(--fg-0);
    line-height: 1.55;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .assistant-text :global(pre) {
    position: relative;
    max-width: 100%;
  }
  .assistant-text :global(:not(pre) > code) {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .assistant-text :global(.copy-code) {
    position: absolute;
    top: 4px;
    right: 6px;
    padding: 1px 6px;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--fg-3);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    opacity: 0;
    transition: opacity var(--motion);
  }
  .assistant-text :global(pre:hover .copy-code),
  .assistant-text :global(.copy-code:focus-visible) {
    opacity: 1;
  }
  .assistant-text :global(.copy-code:hover) {
    color: var(--fg-0);
  }
  .assistant-text :global(p) {
    margin: 0 0 6px;
  }
  .assistant-text :global(p:last-child) {
    margin-bottom: 0;
  }
  .assistant-text :global(h1),
  .assistant-text :global(h2),
  .assistant-text :global(h3),
  .assistant-text :global(h4) {
    margin: 10px 0 4px;
    font-size: 1.05em;
    font-weight: 650;
    color: var(--fg-0);
  }
  .assistant-text :global(h1:first-child),
  .assistant-text :global(h2:first-child),
  .assistant-text :global(h3:first-child),
  .assistant-text :global(h4:first-child) {
    margin-top: 0;
  }
  .assistant-text :global(ul),
  .assistant-text :global(ol) {
    margin: 4px 0 8px;
    padding-left: 22px;
  }
  .assistant-text :global(li) {
    margin: 2px 0;
  }
  .assistant-text :global(code) {
    font-family: var(--font-mono);
    font-size: 0.88em;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 1px 5px;
  }
  .assistant-text :global(pre) {
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre;
    overflow-x: auto;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    margin: 6px 0 10px;
  }
  .assistant-text :global(pre code) {
    background: none;
    border: none;
    padding: 0;
    font-size: inherit;
  }
  .assistant-text :global(blockquote) {
    margin: 6px 0;
    padding: 2px 12px;
    border-left: 2px solid var(--border-strong);
    color: var(--fg-2);
  }
  .assistant-text :global(a) {
    color: var(--accent);
  }
  .assistant-text :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 10px 0;
  }
  .assistant-text :global(del) {
    color: var(--fg-2);
  }
  .caret {
    display: inline-block;
    width: 7px;
    height: 13px;
    margin-left: 2px;
    vertical-align: -2px;
    background: var(--accent);
    animation: blink 1s steps(2) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .thinking {
    border-left: 2px solid var(--border-strong);
    padding-left: 10px;
  }
  .thinking-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
    max-width: 100%;
    text-align: left;
    font-size: 11px;
    color: var(--fg-3);
    padding: 2px 0;
  }
  .thinking-head:hover {
    color: var(--fg-1);
  }
  .thinking-head .req {
    margin-left: 4px;
  }
  .chev {
    width: 10px;
  }
  .live {
    color: var(--accent);
  }
  .thinking-body {
    font-size: 12px;
    color: var(--fg-2);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding: 4px 0 6px;
  }
  .error-msg {
    font-size: 12px;
    line-height: 1.5;
    color: var(--err);
    white-space: pre-wrap;
    word-break: break-word;
    padding: 6px 10px;
    border-left: 2px solid var(--err);
    background: var(--bg-1);
    border-radius: 0 var(--radius) var(--radius) 0;
  }
</style>
