<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import FilePicker from './FilePicker.svelte';
  import ModelPicker from './ModelPicker.svelte';
  import ThinkingMenu from './ThinkingMenu.svelte';
  import PlanBar from './PlanBar.svelte';

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
    if (e.key === 'ArrowUp' && session.draft.length === 0) {
      const last = session.lastUserText();
      if (last) {
        e.preventDefault();
        session.draft = last;
        queueMicrotask(autoGrow);
      }
      return;
    }
    if (e.key === '/' && session.draft.length === 0) {
      e.preventDefault();
      paletteOpen = true;
      return;
    }
    if (e.key === '@') {
      session.draft = session.draft.slice(0, -1);
      pickerOpen = true;
    }
  }
</script>

<PlanBar />
<div class="composer-wrap">
  {#if session.queue.length > 0}
    <ul class="queue" aria-label="Queued messages">
      {#each session.queue as q, i}
        <li class="queued rise">
          <span class="q-tag">queued</span>
          <span class="q-text">{q.text}</span>
          {#if q.pins.length > 0}
            <span class="q-pins mono" title={q.pins.map((p) => p.path).join('\n')}>
              +{q.pins.length} file{q.pins.length === 1 ? '' : 's'}
            </span>
          {/if}
          <button class="q-x" onclick={() => session.removeQueued(i)} aria-label="Remove queued message">
            <Icon name="x" size={10} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
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

  <div class="composer" class:busy={session.running} class:planning={session.planMode}>
    {#if session.pinned.length > 0}
      <ul class="pins" aria-label="Attached files">
        {#each session.pinned as p (p.path)}
          <li class="pin-chip">
            <Icon name="file" size={10} />
            <span class="pin-path mono" title={p.path}>
              {p.path}{#if p.start} · {p.start}{#if p.end && p.end !== p.start}-{p.end}{/if}{/if}
            </span>
            <button
              class="pin-x"
              onclick={() => session.unpin(p.path)}
              aria-label={`Detach ${p.path}`}
              title="Remove from this message"
            >
              <Icon name="x" size={9} />
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    <textarea
      bind:this={ta}
      bind:value={session.draft}
      oninput={autoGrow}
      onkeydown={onKeydown}
      placeholder={session.running
        ? 'running… type to queue the next message (Esc to cancel)'
        : session.planMode
          ? 'Describe what to plan — nothing is changed until you approve…'
          : 'Ask hate-agent…'}
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
      <button
        class="plan-toggle"
        class:on={session.planMode}
        onclick={() => session.togglePlan()}
        aria-pressed={session.planMode}
        title="Plan mode: read-only investigation, then a plan you approve (/plan)"
      >
        plan
      </button>
      <ModelPicker />
      <ThinkingMenu />
      {#if session.running}
        <button class="send cancel" onclick={() => session.cancel()} aria-label="Cancel turn" title="Cancel (Esc)">
          <Icon name="x" size={13} />
        </button>
        {#if session.draft.trim()}
          <button class="send" onclick={() => session.send()} aria-label="Queue message" title="Queue (Enter)">
            <Icon name="plus" size={13} />
          </button>
        {/if}
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
    flex: none;
    min-width: 0;
    container-type: inline-size;
  }
  @container (max-width: 560px) {
    .hint {
      display: none;
    }
  }
  .prompt-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    margin-bottom: 8px;
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
  }
  .prompt-text {
    color: var(--warn);
    font-size: 12px;
    flex: 1 1 200px;
    min-width: 0;
    overflow-wrap: anywhere;
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
  .composer.planning {
    border-style: dashed;
    border-color: var(--accent-dim);
  }
  .plan-toggle {
    padding: 2px 8px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    flex: none;
  }
  .plan-toggle:hover {
    color: var(--fg-1);
  }
  .plan-toggle.on {
    color: var(--accent);
    border-color: var(--accent-dim);
  }
  .queue {
    list-style: none;
    margin: 0 0 6px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 96px;
    overflow-y: auto;
  }
  .queued {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    background: var(--bg-1);
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    font-size: 12px;
    flex: none;
    min-width: 0;
  }
  .q-tag {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
    flex: none;
  }
  .q-text {
    flex: 1;
    min-width: 0;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .q-pins {
    flex: none;
    font-size: 10px;
    color: var(--accent);
  }
  .pins {
    list-style: none;
    margin: 0;
    padding: 6px 8px 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .pin-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 240px;
    padding: 2px 5px 2px 6px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--fg-2);
  }
  .pin-path {
    font-size: 10.5px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pin-x {
    display: inline-flex;
    color: var(--fg-3);
    flex: none;
  }
  .pin-x:hover {
    color: var(--err);
  }
  .q-x {
    display: inline-flex;
    color: var(--fg-3);
    flex: none;
  }
  .q-x:hover {
    color: var(--err);
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
  .composer-meta > :global(.pop:not(.pop-left)) {
    flex: 0 1 auto;
    min-width: 56px;
    max-width: 240px;
  }
  .composer-meta > :global(.pop:not(.pop-left) .pop-trigger) {
    min-width: 0;
    max-width: 100%;
  }
  .hint {
    font-size: 10.5px;
    color: var(--fg-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 0 1 auto;
    min-width: 0;
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
