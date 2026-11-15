<script lang="ts">
  import { session, type SessionMode } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type { SessionMeta } from '../ipc';

  interface Props {
    mode: SessionMode;
  }

  let { mode }: Props = $props();

  const noun = $derived(mode === 'code' ? 'code session' : 'chat');

  function relTime(sec: number): string {
    const diff = Math.max(0, Date.now() / 1000 - sec);
    if (diff < 60) return 'just now';
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    if (diff < 86400 * 7) return `${Math.floor(diff / 86400)}d ago`;
    return new Date(sec * 1000).toLocaleDateString(undefined, {
      month: 'short',
      day: 'numeric',
    });
  }

  function metaLine(s: SessionMeta): string {
    const parts: string[] = [];
    if (s.model) parts.push(s.model);
    if (s.provider) parts.push(s.provider);
    if (mode === 'code' && s.workspace) {
      parts.push(s.workspace.split(/[\\/]/).filter(Boolean).pop() ?? '');
    }
    return parts.filter(Boolean).join(' · ');
  }

  async function open(s: SessionMeta): Promise<void> {
    await session.openSession(s.id);
  }

  function focusOnMount(node: HTMLInputElement): void {
    node.focus();
    node.select();
  }

  let query = $state('');
  let renaming = $state<string | null>(null);
  let draftTitle = $state('');

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return session.sessions;
    return session.sessions.filter((s) =>
      [s.title, s.model, s.provider, s.workspace ?? '']
        .join(' ')
        .toLowerCase()
        .includes(q),
    );
  });

  function startRename(s: SessionMeta): void {
    renaming = s.id;
    draftTitle = s.title || '';
  }

  async function commitRename(): Promise<void> {
    const id = renaming;
    if (!id) return;
    const title = draftTitle.trim();
    renaming = null;
    if (!title) return;
    await session.renameSession(id, title);
  }

  function onRenameKey(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      e.preventDefault();
      void commitRename();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      renaming = null;
    }
  }
</script>

<div class="browser">
  <div class="browser-inner">
    <button class="new-card" onclick={() => void session.newSession()} disabled={session.sessionBusy}>
      <span class="new-icon"><Icon name="plus" size={16} /></span>
      <span class="new-text">
        <span class="new-title">New {noun}</span>
        <span class="new-sub">fresh context, same providers</span>
      </span>
      <Icon name="chevron-right" size={14} />
    </button>

    <div class="list-head">
      <span class="list-title">Recent {mode === 'code' ? 'code sessions' : 'chats'}</span>
      {#if session.sessions.length > 0}
        <input
          class="search"
          type="search"
          bind:value={query}
          placeholder="filter…"
          aria-label="Filter sessions"
        />
      {/if}
      <button
        class="refresh"
        onclick={() => void session.refreshSessions(mode)}
        title="Refresh"
        aria-label="Refresh sessions"
      >
        <Icon name="clock" size={12} />
      </button>
    </div>

    {#if session.sessionsState === 'loading' && session.sessions.length === 0}
      <ul class="list skeleton" aria-hidden="true">
        {#each [0, 1, 2] as i (i)}
          <li class="row skel-row" style:opacity={1 - i * 0.22}>
            <span class="skel skel-title"></span>
            <span class="skel skel-meta"></span>
          </li>
        {/each}
      </ul>
      <p class="sr-only" role="status">loading sessions</p>
    {:else if session.sessionsState === 'error'}
      <p class="state-line err">couldn't load sessions</p>
    {:else if session.sessions.length === 0}
      <p class="state-line">
        No {mode === 'code' ? 'code sessions' : 'chats'} yet — everything you
        start is saved automatically from here on.
      </p>
    {:else if filtered.length === 0}
      <p class="state-line">No sessions match “{query}”.</p>
    {:else}
      <ul class="list">
        {#each filtered as s (s.id)}
          <li class="row" class:active={s.id === session.sessionId}>
            {#if renaming === s.id}
              <div class="row-main">
                <input
                  class="rename-input"
                  bind:value={draftTitle}
                  onkeydown={onRenameKey}
                  onblur={() => void commitRename()}
                  aria-label="Session title"
                  use:focusOnMount
                />
                <span class="row-meta mono">enter to save · esc to cancel</span>
              </div>
            {:else}
              <button class="row-main" onclick={() => void open(s)}>
                <span class="row-title">{s.title || 'Untitled'}</span>
                <span class="row-meta mono">
                  {metaLine(s)}{metaLine(s) ? ' · ' : ''}{relTime(s.updated_at)}
                </span>
              </button>
            {/if}
            <button
              class="row-edit"
              onclick={() => startRename(s)}
              aria-label="Rename session"
              title="Rename session"
            >
              <Icon name="edit" size={13} />
            </button>
            <button
              class="row-del"
              onclick={() => void session.deleteSession(s.id)}
              aria-label="Delete session"
              title="Delete session"
            >
              <Icon name="trash" size={13} />
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .browser {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    background: var(--bg-0);
  }
  .browser-inner {
    max-width: 640px;
    margin: 0 auto;
    padding: 48px 24px 64px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .new-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 16px;
    text-align: left;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    cursor: pointer;
    transition:
      border-color var(--motion),
      transform var(--motion);
  }
  .new-card:hover {
    border-color: var(--accent-dim);
    transform: translateY(-1px);
  }
  .new-card:disabled {
    opacity: 0.5;
    cursor: default;
    transform: none;
  }
  .new-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-fg);
    flex: none;
  }
  .new-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .new-title {
    font-size: 13.5px;
    font-weight: 650;
    color: var(--fg-0);
  }
  .new-sub {
    font-size: 11.5px;
    color: var(--fg-3);
  }
  .new-card > :global(svg) {
    color: var(--fg-3);
    flex: none;
  }
  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .list-title {
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .refresh {
    display: inline-flex;
    padding: 4px;
    border-radius: var(--radius-sm);
    color: var(--fg-3);
  }
  .refresh:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .search {
    width: 132px;
    padding: 3px 8px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--fg-1);
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    outline: none;
  }
  .search:focus {
    border-color: var(--border-strong);
  }
  .search::placeholder {
    color: var(--fg-3);
  }
  .rename-input {
    width: 100%;
    padding: 2px 6px;
    font: inherit;
    font-size: 13px;
    color: var(--fg-0);
    background: var(--bg-0);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    outline: none;
  }
  .row-edit {
    flex: none;
    display: inline-flex;
    padding: 8px 6px;
    color: var(--fg-3);
    opacity: 0;
    transition: opacity var(--motion);
  }
  .row:hover .row-edit,
  .row-edit:focus-visible {
    opacity: 1;
  }
  .row-edit:hover {
    color: var(--fg-0);
  }
  .skel-row {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 10px 8px;
    pointer-events: none;
  }
  .skel {
    display: block;
    height: 9px;
    border-radius: 4px;
    background: linear-gradient(
      90deg,
      var(--bg-2) 0%,
      var(--bg-3) 50%,
      var(--bg-2) 100%
    );
    background-size: 200% 100%;
    animation: skel-shimmer 1.3s ease-in-out infinite;
  }
  .skel-title {
    width: 62%;
  }
  .skel-meta {
    width: 38%;
    height: 7px;
    opacity: 0.6;
  }
  @keyframes skel-shimmer {
    0% {
      background-position: 140% 0;
    }
    100% {
      background-position: -40% 0;
    }
  }
  .state-line {
    font-size: 12.5px;
    color: var(--fg-3);
    padding: 8px 2px;
  }
  .state-line.err {
    color: var(--err);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: center;
    border-radius: var(--radius);
  }
  .row:hover {
    background: var(--bg-1);
  }
  .row.active {
    background: var(--bg-1);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 9px 12px;
    text-align: left;
    cursor: pointer;
  }
  .row-title {
    font-size: 13px;
    color: var(--fg-0);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-meta {
    font-size: 10.5px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-del {
    flex: none;
    display: inline-flex;
    padding: 8px 10px;
    color: var(--fg-3);
    opacity: 0;
    transition: opacity var(--motion);
  }
  .row:hover .row-del,
  .row-del:focus-visible {
    opacity: 1;
  }
  .row-del:hover {
    color: var(--err);
  }
</style>
