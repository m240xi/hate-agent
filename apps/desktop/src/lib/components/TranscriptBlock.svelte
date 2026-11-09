<script lang="ts">
  import { session } from '../store.svelte';
  import ToolCard from './ToolCard.svelte';
  import type { TranscriptBlock } from '../types';

  // Renders one transcript block by kind: user message, assistant text
  // (streaming-aware), collapsed thinking, or a tool card.
  interface Props {
    block: TranscriptBlock;
  }

  let { block }: Props = $props();

  let thinkingOpen = $state(false);
</script>

{#if block.kind === 'user'}
  <div class="user-msg rise">
    <div class="user-tag">you</div>
    <div class="user-text">{block.text}</div>
  </div>
{:else if block.kind === 'text'}
  <div class="assistant-text" class:streaming={block.streaming}>
    {block.text}
    {#if block.streaming}<span class="caret" aria-hidden="true"></span>{/if}
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
  .assistant-text {
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--fg-0);
    line-height: 1.55;
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
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--fg-3);
    padding: 2px 0;
  }
  .thinking-head:hover {
    color: var(--fg-1);
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
