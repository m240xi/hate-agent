<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import FilePicker from './FilePicker.svelte';
  import ModelPicker from './ModelPicker.svelte';
  import ThinkingMenu from './ThinkingMenu.svelte';

  // Bottom composer: Enter sends, Shift+Enter newline, Esc cancels,
  // `/` opens the command palette, `@` opens file fuzzy-find,
  // Shift+Tab cycles thinking level.
  let ta = $state<HTMLTextAreaElement | null>(null);
  let paletteOpen = $state(false);
  let pickerOpen = $state(false);

  function autoGrow(): void {
    if (!ta) return;
    ta.style.height = 'auto';
    ta.style.height = `${Math.min(ta.scrollHeight, 180)}px`;
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      session.send();
      return;
    }
    if (e.key === 'Escape') {
      if (session.running) session.cancel();
      return;
    }
    if (e.key === 'Tab' && e.shiftKey) {
      e.preventDefault();
      session.cycleThinking();
      return;
    }
    // `/` at position 0 opens the palette; `@` anywhere opens file picker.
    // The trigger char is consumed — pickers own their own input.
    if (e.key === '/' && session.draft.length === 0) {
      paletteOpen = true;
      return;
    }
    if (e.key === '@') {
      session.draft = session.draft.slice(0, -1);
      pickerOpen = true;
    }
  }
</script>

<div class="composer-wrap">
  {#if session.awaitingPrompt}
    <div class="prompt-bar rise" role="alert">
      <span class="prompt-text">{session.awaitingPrompt}</span>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          const input = e.currentTarget.querySelector('input');
          if (input?.value.trim()) session.answerPrompt(input.value.trim());
        }}
      >
        <input placeholder="answer…" aria-label="Answer prompt" />
      </form>
    </div>
  {/if}

  <div class="composer" class:busy={session.running}>
    <textarea
      bind:this={ta}
      bind:value={session.draft}
      oninput={autoGrow}
      onkeydown={onKeydown}
      placeholder={session.running ? 'running… (Esc to cancel)' : 'Ask hate-agent…'}
      rows="1"
      aria-label="Message composer"
      disabled={session.sessionState === 'awaiting_permission'}
    ></textarea>
    <div class="composer-meta">
      <span class="hint">
        <kbd>⏎</kbd> send · <kbd>⇧⏎</kbd> newline · <kbd>esc</kbd> cancel ·
        <kbd>/</kbd> commands · <kbd>@</kbd> files · <kbd>⇧tab</kbd> think
      </span>
      <span class="spacer"></span>
      {#if session.provider}
        <span class="provider mono" title="active provider">{session.provider}</span>
      {/if}
      <ModelPicker />
      <ThinkingMenu />
      {#if session.running}
        <button class="send cancel" onclick={() => session.cancel()} aria-label="Cancel turn" title="Cancel (Esc)">
          <Icon name="x" size={13} />
        </button>
      {:else}
        <button
          class="send"
          onclick={() => session.send()}
          disabled={!session.draft.trim()}
          aria-label="Send message"
          title="Send (Enter)"
        >
          <Icon name="send" size={13} />
        </button>
      {/if}
    </div>
  </div>

  {#if paletteOpen}
    <CommandPalette onclose={() => (paletteOpen = false)} />
  {/if}
  {#if pickerOpen}
    <FilePicker onclose={() => (pickerOpen = false)} />
  {/if}
</div>

<style>
  .composer-wrap {
    padding: 8px 16px 12px;
    background: var(--bg-0);
  }
  .prompt-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
  }
  .prompt-text {
    color: var(--warn);
    font-size: 12px;
    flex: 1;
    min-width: 0;
  }
  .prompt-bar input {
    width: 200px;
    padding: 4px 8px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-size: 12px;
  }
  .composer {
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.25);
    transition: border-color var(--motion);
  }
  .composer:focus-within {
    border-color: var(--accent-dim);
  }
  .composer textarea {
    display: block;
    width: 100%;
    resize: none;
    background: none;
    border: none;
    outline: none;
    padding: 10px 12px 4px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--fg-0);
    max-height: 180px;
  }
  .composer-meta {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px 6px 12px;
  }
  .hint {
    font-size: 10.5px;
    color: var(--fg-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    padding: 1px 4px;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-2);
  }
  .spacer {
    flex: 1;
  }
  .provider {
    font-size: 10.5px;
    color: var(--fg-3);
    flex: none;
  }
  .send {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-fg);
    flex: none;
  }
  .send:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .send.cancel {
    background: var(--err);
    color: #fff;
  }
</style>
