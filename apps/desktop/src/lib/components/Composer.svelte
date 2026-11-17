<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import FilePicker from './FilePicker.svelte';
  import ModelPicker from './ModelPicker.svelte';
  import ThinkingMenu from './ThinkingMenu.svelte';
  import PlanBar from './PlanBar.svelte';
  import type { ContextRef } from '../types';

  let ta = $state<HTMLTextAreaElement | null>(null);
  let wrap = $state<HTMLDivElement | null>(null);
  let wrapCompact = $state(false);
  let wrapTight = $state(false);
  let paletteOpen = $state(false);
  let pickerOpen = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
  let dragging = $state(false);
  const folderName = $derived(
    session.workspacePath?.split(/[\\/]/).filter(Boolean).pop() ?? '',
  );

  function ctxIcon(c: ContextRef): string {
    if (c.kind === 'git_diff' || c.kind === 'git_staged') return 'diff';
    if (c.kind === 'terminal') return 'terminal';
    if (c.kind === 'url') return 'globe';
    return 'folder';
  }

  function ctxLabel(c: ContextRef): string {
    if (c.kind === 'git_diff') return '@git:diff';
    if (c.kind === 'git_staged') return '@git:staged';
    if (c.kind === 'terminal') return '@terminal';
    if (c.kind === 'url') return `@url ${c.url}`;
    return `@folder:${c.path}`;
  }

  // Measure the composer width via ResizeObserver rather than `container-type:
  // inline-size`: CSS layout containment creates both a stacking context (trapping
  // popovers below `.glass-pane::after`) and a Backdrop Root (blocking glass
  // `backdrop-filter` blur on the transcript behind popovers).
  $effect(() => {
    const el = wrap;
    if (!el) return;
    const update = (w: number): void => {
      wrapCompact = w <= 560;
      wrapTight = w <= 400;
    };
    const cs = getComputedStyle(el);
    const padX = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
    update(el.clientWidth - padX);
    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const w = entry.contentBoxSize?.[0]?.inlineSize ?? entry.contentRect.width;
      update(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  function onPaste(e: ClipboardEvent): void {
    const files = [...(e.clipboardData?.items ?? [])]
      .filter((i) => i.kind === 'file' && i.type.startsWith('image/'))
      .map((i) => i.getAsFile())
      .filter((f): f is File => f !== null);
    if (files.length === 0) return;
    e.preventDefault();
    void Promise.all(files.map((f) => session.addImage(f)));
  }

  function onDrop(e: DragEvent): void {
    dragging = false;
    const files = [...(e.dataTransfer?.files ?? [])].filter((f) => f.type.startsWith('image/'));
    if (files.length === 0) return;
    e.preventDefault();
    void Promise.all(files.map((f) => session.addImage(f)));
  }

  function onFilePick(e: Event): void {
    const input = e.currentTarget as HTMLInputElement;
    const files = [...(input.files ?? [])];
    void Promise.all(files.map((f) => session.addImage(f)));
    input.value = '';
  }

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
    if (e.key === '@' && (session.draft.length === 0 || /\s$/.test(session.draft))) {
      e.preventDefault();
      pickerOpen = true;
    }
  }
</script>

<PlanBar />
<div
  class="composer-wrap"
  class:compact={wrapCompact}
  class:tight={wrapTight}
  bind:this={wrap}
>
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

  <div
    class="composer"
    class:busy={session.running}
    class:planning={session.planMode}
    class:dragging
    role="group"
    aria-label="Message composer"
    ondragover={(e) => {
      if ([...(e.dataTransfer?.items ?? [])].some((i) => i.type.startsWith('image/'))) {
        e.preventDefault();
        dragging = true;
      }
    }}
    ondragleave={() => (dragging = false)}
    ondrop={onDrop}
  >
    {#if session.attachments.length > 0}
      <ul class="attachments" aria-label="Attached images">
        {#each session.attachments as a, i (a.url)}
          <li class="attach">
            <img src={a.url} alt="" />
            <button
              class="attach-x"
              onclick={() => session.removeAttachment(i)}
              aria-label="Remove attached image"
              title="Remove"
            >
              <Icon name="x" size={9} />
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if session.visionUnsupported && session.attachments.length > 0}
      <p class="vision-warn">
        {session.model} can't see images — the attachment will be ignored by the model.
      </p>
    {/if}
    {#if session.pinned.length > 0 || session.contexts.length > 0}
      <ul class="pins" aria-label="Attached context">
        {#each session.contexts as c, ci}
          <li class="pin-chip ctx">
            <Icon name={ctxIcon(c)} size={10} />
            <span class="pin-path mono" title={ctxLabel(c)}>{ctxLabel(c)}</span>
            <button
              class="pin-x"
              onclick={() => session.removeContext(ci)}
              aria-label={`Detach ${ctxLabel(c)}`}
              title="Remove from this message"
            >
              <Icon name="x" size={9} />
            </button>
          </li>
        {/each}
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
      onpaste={onPaste}
      placeholder={session.running
        ? 'Working… type to queue a follow-up (Esc to stop)'
        : session.planMode
          ? 'Describe what to plan — nothing changes until you approve'
          : session.workspacePath
            ? 'Describe a change ? / for commands, @ for files & context'
            : 'Ask anything ? / for commands, @ for terminal or URL'}
      rows="1"
      aria-label="Message composer"
      disabled={session.sessionState === 'awaiting_permission'}
    ></textarea>
    <div class="composer-meta">
      <button
        class="attach-btn"
        onclick={() => fileInput?.click()}
        aria-label="Attach image"
        title="Attach an image (or paste / drop one)"
      >
        <Icon name="plus" size={16} />
      </button>
      <input
        class="file-input"
        type="file"
        accept="image/png,image/jpeg,image/webp,image/gif,image/bmp"
        multiple
        bind:this={fileInput}
        onchange={onFilePick}
      />
      {#if session.workspacePath}
        <span class="folder-chip" title={session.workspacePath}>
          <button
            class="folder-main"
            onclick={() => session.pickWorkspace()}
            disabled={session.running}
            aria-label="Change folder"
          >
            <Icon name={session.worktree?.is_worktree ? 'branch' : 'folder'} size={13} />
            <span class="folder-name">
              {folderName}{#if session.worktree?.is_worktree} ? wt{/if}
            </span>
          </button>
          <button
            class="folder-x"
            onclick={() => session.attachFolder(null)}
            disabled={session.running}
            aria-label="Remove folder from this chat"
            title="Remove folder from this chat"
          >
            <Icon name="x" size={11} />
          </button>
        </span>
      {:else}
        <button
          class="plan-toggle"
          onclick={() => session.pickWorkspace()}
          disabled={session.running}
          title="Let this chat read and edit a project folder"
        >
          <Icon name="folder" size={13} />
          <span class="lbl">Folder</span>
        </button>
      {/if}
      <button
        class="plan-toggle"
        class:on={session.planMode}
        onclick={() => session.togglePlan()}
        aria-pressed={session.planMode}
        title="Plan mode: read-only investigation, then a plan you approve (/plan)"
      >
        <Icon name="list-ordered" size={13} />
        <span class="lbl">Plan</span>
      </button>
      <span class="spacer"></span>
      <ModelPicker />
      <ThinkingMenu />
      {#if session.running}
        {#if session.draft.trim()}
          <button class="send" onclick={() => session.send()} aria-label="Queue message" title="Queue (Enter)">
            <Icon name="arrow-up" size={15} />
          </button>
        {/if}
        <button class="send cancel" onclick={() => session.cancel()} aria-label="Stop" title="Stop (Esc)">
          <Icon name="stop" size={12} />
        </button>
      {:else}
        <button
          class="send"
          onclick={() => session.send()}
          disabled={!session.draft.trim() && session.attachments.length === 0 && session.contexts.length === 0 && session.pinned.length === 0}
          aria-label="Send message"
          title="Send (Enter) · Shift+Enter for a new line"
        >
          <Icon name="arrow-up" size={15} />
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
    padding: 6px 16px 4px;
    background: var(--bg-0);
    flex: none;
    min-width: 0;
  }
  .composer-wrap.compact .lbl {
    display: none;
  }
  .composer-wrap.compact .plan-toggle {
    width: 32px;
    padding: 0;
    justify-content: center;
  }
  .composer-wrap.compact .folder-chip {
    max-width: 120px;
  }
  .composer-wrap.compact .composer-meta > :global(.mp-pop) {
    max-width: 150px;
  }
  .composer-wrap.tight .folder-name {
    display: none;
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
    border-radius: 22px;
    box-shadow: var(--el-2);
    transition:
      border-color var(--motion),
      box-shadow var(--motion);
  }
  .composer:focus-within {
    border-color: color-mix(in srgb, var(--accent) 30%, var(--border-strong));
    box-shadow:
      var(--shadow-md),
      0 0 0 3px var(--accent-soft);
  }
  .composer.planning {
    border-style: dashed;
    border-color: var(--accent-dim);
  }
  .composer.dragging {
    border-color: var(--accent-dim);
    background: var(--accent-soft);
  }
  .attachments {
    list-style: none;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 12px 14px 0;
  }
  .attach {
    position: relative;
    width: 56px;
    height: 56px;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-0);
  }
  .attach img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .attach-x {
    position: absolute;
    top: 2px;
    right: 2px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--bg-0) 75%, transparent);
    color: var(--fg-1);
  }
  .attach-x:hover {
    color: var(--err);
  }
  .vision-warn {
    margin: 6px 10px 0;
    font-size: 10.5px;
    color: var(--warn);
  }
  .attach-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    color: var(--fg-2);
    border: 1px solid var(--border);
    flex: none;
  }
  .attach-btn:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }
  .file-input {
    display: none;
  }
  .plan-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 12px;
    font-size: 12.5px;
    color: var(--fg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    flex: none;
  }
  .plan-toggle:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }
  .plan-toggle:disabled,
  .folder-chip button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .folder-chip {
    display: inline-flex;
    align-items: center;
    height: 32px;
    max-width: 200px;
    flex: 0 1 auto;
    min-width: 0;
    color: var(--fg-0);
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--border));
    background: var(--accent-soft);
    border-radius: var(--radius-pill);
  }
  .folder-main {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 100%;
    padding: 0 4px 0 12px;
    font-size: 12.5px;
  }
  .folder-name {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .folder-x {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 100%;
    padding-right: 4px;
    color: var(--fg-2);
    border-radius: 0 var(--radius-pill) var(--radius-pill) 0;
  }
  .folder-x:hover:not(:disabled) {
    color: var(--err);
  }
  .plan-toggle.on {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-soft);
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
  .pin-chip.ctx {
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
    background: var(--accent-soft);
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
    padding: 14px 18px 6px;
    font-size: 14px;
    line-height: 1.5;
    color: var(--fg-0);
    max-height: 220px;
  }
  .composer textarea::placeholder {
    color: var(--fg-3);
  }
  .composer-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px 8px 10px;
  }
  .composer-meta > :global(.mp-pop) {
    flex: 0 1 auto;
    min-width: 56px;
    max-width: 240px;
  }
  .composer-meta > :global(.mp-pop .pop-trigger) {
    min-width: 0;
    max-width: 100%;
  }
  .spacer {
    flex: 1;
  }
  .send {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-fg);
    flex: none;
  }
  .send:disabled {
    opacity: 0.3;
    cursor: default;
  }
  .send.cancel {
    background: var(--fg-0);
    color: var(--bg-0);
  }
</style>
