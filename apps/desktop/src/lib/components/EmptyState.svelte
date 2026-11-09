<script lang="ts">
  // Empty state — not a hero image. How this works plus the three most
  // useful commands. Provider setup lives in Settings; with more
  // providers coming this view stays provider-agnostic.
  import { session } from '../store.svelte';
</script>

<div class="empty">
  <div class="wordmark">hate-agent</div>
  <p class="line">
    A coding agent that spends fewer tokens per task — small prompt, lazy
    tools, shaped output, verified work.
  </p>
  {#if session.providers.length === 0}
    <p class="no-provider">
      No providers connected yet — add one in <strong>Settings</strong> to
      start chatting.
    </p>
  {/if}
  <ol class="how">
    <li>Describe a task in the composer below.</li>
    <li>The agent reads, edits, and runs checks — every tool call shows its token cost.</li>
    <li>It verifies before claiming done; you see the real check output.</li>
  </ol>
  <div class="cmds">
    <span class="cmd"><kbd>/</kbd> commands</span>
    <span class="cmd"><kbd>@</kbd> attach file</span>
    <span class="cmd"><kbd>esc</kbd> cancel turn</span>
  </div>
</div>

<style>
  .empty {
    max-width: 520px;
    margin: 12vh auto 0;
    padding: 0 24px;
    color: var(--fg-2);
  }
  .wordmark {
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--fg-0);
    margin-bottom: 10px;
  }
  .line {
    font-size: 13px;
    line-height: 1.6;
    margin: 0 0 16px;
  }
  .no-provider {
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--fg-3);
    margin: 0 0 16px;
  }
  .no-provider strong {
    color: var(--fg-1);
    font-weight: 600;
  }
  .how {
    margin: 0 0 20px;
    padding-left: 20px;
    font-size: 12.5px;
    line-height: 1.7;
  }
  .how li {
    margin-bottom: 2px;
  }
  .cmds {
    display: flex;
    gap: 16px;
    font-size: 11.5px;
  }
  .cmd {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-3);
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    padding: 1px 5px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-2);
  }
</style>
