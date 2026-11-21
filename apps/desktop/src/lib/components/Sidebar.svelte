<script lang="ts">
  import { session } from '../store.svelte';
  import type { SessionMeta } from '../ipc';
  import Icon from './Icon.svelte';
  import { dragRegion } from '../window';
  import { onMount } from 'svelte';
  import { fly, slide } from 'svelte/transition';
  import { SETTINGS_SECTIONS } from '../settings';
  import { cubicOut } from 'svelte/easing';
  import { ms } from '../motion';
  import PixelCat from './PixelCat.svelte';
  import ProviderLogo from './ProviderLogo.svelte';

  let query = $state('');
  let searching = $state(false);
  let renaming = $state<string | null>(null);
  let draftTitle = $state('');
  let searchEl = $state<HTMLInputElement | null>(null);
  let compareOpen = $state(false);
  let comparePrompt = $state('');
  let compareA = $state('');
  let compareB = $state('');

  function isWorktreeSession(s: SessionMeta): boolean {
    return (
      Boolean(s.workspace?.includes('.hate-worktrees')) ||
      Boolean(s.workspace?.includes('/worktrees/')) ||
      Boolean(s.workspace?.includes('\\worktrees\\'))
    );
  }

  function openCompare(): void {
    compareOpen = !compareOpen;
    if (compareOpen && session.models.length > 0) {
      compareA = compareA || session.models[0]?.id || session.model;
      compareB = compareB || session.models[1]?.id || session.models[0]?.id || session.model;
    }
  }

  async function runCompare(): Promise<void> {
    const pick = (id: string) => {
      const found = session.models.find((m) => m.id === id);
      return {
        provider: found?.provider || session.providerName || session.provider,
        model: found?.id || id || session.model,
      };
    };
    await session.compareModelsInWorktrees(comparePrompt, [pick(compareA), pick(compareB)]);
    compareOpen = false;
    comparePrompt = '';
  }

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return session.sessions;
    return session.sessions.filter((s) =>
      [s.title, s.model, s.provider, s.workspace ?? ''].join(' ').toLowerCase().includes(q),
    );
  });

  const groups = $derived.by(() => {
    const day = 86400;
    const now = new Date();
    const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
    const buckets: { label: string; items: SessionMeta[] }[] = [
      { label: 'Today', items: [] },
      { label: 'Yesterday', items: [] },
      { label: 'Previous 7 days', items: [] },
      { label: 'Previous 30 days', items: [] },
      { label: 'Older', items: [] },
    ];
    for (const s of filtered) {
      const t = s.updated_at;
      const i =
        t >= midnight ? 0 : t >= midnight - day ? 1 : t >= midnight - 7 * day ? 2 : t >= midnight - 30 * day ? 3 : 4;
      buckets[i]!.items.push(s);
    }
    return buckets.filter((b) => b.items.length > 0);
  });

  function workspaceOf(s: SessionMeta): string {
    return s.workspace?.split(/[\\/]/).filter(Boolean).pop() ?? '';
  }

  function tooltip(s: SessionMeta): string {
    return [s.title || 'Untitled', [s.model, s.provider].filter(Boolean).join(' · '), workspaceOf(s)]
      .filter(Boolean)
      .join('\n');
  }

  function openSearch(): void {
    searching = true;
    queueMicrotask(() => searchEl?.focus());
  }

  function closeSearch(): void {
    searching = false;
    query = '';
  }

  function startRename(s: SessionMeta): void {
    renaming = s.id;
    draftTitle = s.title || '';
  }

  async function commitRename(): Promise<void> {
    const id = renaming;
    if (!id) return;
    const title = draftTitle.trim();
    renaming = null;
    if (title) await session.renameSession(id, title);
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

  function focusOnMount(node: HTMLInputElement): void {
    node.focus();
    node.select();
  }

  const active = (s: SessionMeta) =>
    s.id === session.sessionId && session.mode !== 'settings';

  // Renaming lives here, so the app-wide context menu asks for it by event.
  onMount(() => {
    const onRename = (e: Event): void => {
      const id = (e as CustomEvent<{ id: string }>).detail?.id;
      const target = id ? session.sessions.find((s) => s.id === id) : undefined;
      if (target) startRename(target);
    };
    window.addEventListener('hate:rename-session', onRename);
    return () => window.removeEventListener('hate:rename-session', onRename);
  });
</script>

<aside
  class="sidebar glass-pane"
  aria-label="Sidebar"
  transition:slide={{ axis: 'x', duration: ms(200), easing: cubicOut }}
>
  <div class="top" role="presentation" onmousedown={dragRegion}>
    <button class="brand" onclick={() => session.setMode('home')} title="Home">hate-agent</button>
    <span class="grow"></span>
    <button
      class="icon-btn"
      onclick={() => (session.sidebarCollapsed = true)}
      aria-label="Close sidebar"
      title="Close sidebar (Ctrl+B)"
    >
      <Icon name="panel-left" size={16} />
    </button>
  </div>

  {#if session.mode === 'settings'}
  <div class="view" in:fly={{ x: 18, duration: ms(220), easing: cubicOut }}>
    <nav class="actions" aria-label="Back">
      <button class="action" onclick={() => session.setMode('chat')}>
        <Icon name="chevron-left" size={15} />
        <span>Back to chats</span>
      </button>
    </nav>
    <h3 class="group-label settings-label">Settings</h3>
    <nav class="sections" aria-label="Settings sections">
      {#each SETTINGS_SECTIONS as sec (sec.id)}
        <button
          class="action"
          class:current={session.settingsSection === sec.id}
          aria-current={session.settingsSection === sec.id ? 'page' : undefined}
          onclick={() => (session.settingsSection = sec.id)}
        >
          <Icon name={sec.icon} size={15} />
          <span>{sec.label}</span>
        </button>
      {/each}
    </nav>
  </div>
  {:else}
  <div class="view" in:fly={{ x: -18, duration: ms(220), easing: cubicOut }}>
  <nav class="actions" aria-label="Start">
    <button
      class="action"
      class:current={session.mode === 'chat' && !session.sessionId}
      onclick={() => session.startFresh()}
    >
      <Icon name="square-pen" size={15} />
      <span>New chat</span>
      <kbd>Ctrl N</kbd>
    </button>
    {#if searching}
      <div class="search">
        <Icon name="search" size={14} />
        <input
          bind:this={searchEl}
          bind:value={query}
          placeholder="Search chats"
          aria-label="Search chats"
          onkeydown={(e) => e.key === 'Escape' && closeSearch()}
          onblur={() => !query && closeSearch()}
        />
        <button class="search-x" onclick={closeSearch} aria-label="Clear search">
          <Icon name="x" size={12} />
        </button>
      </div>
    {:else}
      <button class="action" onclick={openSearch}>
        <Icon name="search" size={15} />
        <span>Search chats</span>
      </button>
    {/if}
    {#if session.workspacePath && session.worktree?.is_git}
      <button
        class="action"
        onclick={() => void session.spawnWorktreeTrack()}
        title="Create a new isolated git worktree session track"
      >
        <Icon name="branch" size={15} />
        <span>New worktree track</span>
      </button>
      {#if session.models.length >= 2}
        <button
          class="action"
          onclick={openCompare}
          title="Spawn two isolated worktree tracks to compare models"
        >
          <Icon name="branch" size={15} />
          <span>Compare models</span>
        </button>
      {/if}
      {#if compareOpen}
        <div class="compare-box">
          <select class="cmp-sel mono" bind:value={compareA} aria-label="Model A">
            {#each session.models as m (m.id + (m.provider ?? ''))}
              <option value={m.id}>{m.id}</option>
            {/each}
          </select>
          <select class="cmp-sel mono" bind:value={compareB} aria-label="Model B">
            {#each session.models as m (m.id + (m.provider ?? ''))}
              <option value={m.id}>{m.id}</option>
            {/each}
          </select>
          <input
            class="cmp-in"
            bind:value={comparePrompt}
            placeholder="Shared prompt…"
            aria-label="Prompt for model comparison"
          />
          <button class="cmp-go mono" onclick={() => void runCompare()}>Spawn tracks</button>
        </div>
      {/if}
    {/if}
  </nav>

  <div class="history">
    {#if session.sessionsState === 'loading' && session.sessions.length === 0}
      <div class="skel-list" aria-hidden="true">
        {#each [0, 1, 2, 3] as i (i)}
          <span class="skel" style:width={`${78 - i * 12}%`}></span>
        {/each}
      </div>
    {:else if session.sessionsState === 'error'}
      <p class="empty err">Couldn't load your history.</p>
    {:else if session.sessions.length === 0}
      <div class="nap">
        <PixelCat size={64} />
        <p>Nothing here yet.<br />Your chats will curl up here.</p>
      </div>
    {:else if filtered.length === 0}
      <p class="empty">Nothing matches “{query}”.</p>
    {:else}
      {#each groups as g (g.label)}
        <section class="group">
          <h3 class="group-label">{g.label}</h3>
          <ul>
            {#each g.items as s (s.id)}
              <li
                class="row"
                class:active={active(s)}
                data-ctx="session"
                data-id={s.id}
                data-title={s.title || 'Untitled'}
              >
                {#if renaming === s.id}
                  <input
                    class="rename"
                    bind:value={draftTitle}
                    onkeydown={onRenameKey}
                    onblur={() => void commitRename()}
                    aria-label="Rename"
                    use:focusOnMount
                  />
                {:else}
                  <button
                    class="row-main"
                    onclick={() => void session.openFromHistory(s)}
                    ondblclick={() => startRename(s)}
                    title={tooltip(s)}
                  >
                    <span class="row-title">{s.title || 'Untitled'}</span>
                    {#if s.workspace}
                      <span class="tag" title={s.workspace}>
                        <Icon name={isWorktreeSession(s) ? 'branch' : 'folder'} size={10} />
                        {workspaceOf(s)}
                      </span>
                    {/if}
                  </button>
                  <div class="row-actions">
                    <button onclick={() => startRename(s)} aria-label="Rename" title="Rename">
                      <Icon name="edit" size={12} />
                    </button>
                    <button
                      class="del"
                      onclick={() => void session.deleteSession(s.id)}
                      aria-label="Delete"
                      title="Delete"
                    >
                      <Icon name="trash" size={12} />
                    </button>
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    {/if}
  </div>
  </div>
  {/if}

  <div class="foot">
    {#if session.mode !== 'settings'}
      <button class="action" onclick={() => session.setMode('settings')}>
        <Icon name="gear" size={15} />
        <span>Settings</span>
        <kbd>Ctrl ,</kbd>
      </button>
    {/if}
    <div class="who">
      <span class="dot" class:on={session.providers.length > 0}></span>
      {#if session.providerName || session.provider}
        <span class="who-logo" aria-hidden="true">
          <ProviderLogo provider={session.providerName || session.provider} size={12} fallbackFontSize={13} />
        </span>
      {/if}
      <span class="who-text">
        {session.providerName || session.provider || 'No provider yet'}
      </span>
    </div>
  </div>
</aside>

<style>
  .sidebar {
    width: 260px;
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg-1);
    border-right: 1px solid var(--border);
    user-select: none;
    overflow: hidden;
  }
  .sidebar > * {
    min-width: 259px;
  }
  .top {
    display: flex;
    align-items: center;
    height: 48px;
    padding: 0 8px 0 14px;
    flex: none;
  }
  .brand {
    font-family: 'Alagard', var(--font-ui);
    font-size: 17px;
    letter-spacing: 0.03em;
    color: var(--fg-0);
    padding: 3px 6px;
    margin-left: -6px;
    border-radius: var(--radius-sm);
  }
  .brand:hover {
    background: var(--bg-2);
  }
  .grow {
    flex: 1;
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    color: var(--fg-2);
  }
  .icon-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .view {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .sections {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 0 8px;
  }
  .settings-label {
    margin: 6px 18px 6px;
  }
  .actions {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 2px 8px 10px;
    flex: none;
  }
  .action {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px;
    border-radius: 9px;
    font-size: 13px;
    color: var(--fg-1);
    text-align: left;
    transition: background var(--motion), color var(--motion);
  }
  .action > span {
    flex: 1;
  }
  .action:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .action.current {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  kbd {
    font-family: var(--font-ui);
    font-size: 10.5px;
    color: var(--fg-3);
    opacity: 0;
    transition: opacity var(--motion);
  }
  .action:hover kbd {
    opacity: 1;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 6px 0 10px;
    border-radius: 9px;
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    color: var(--fg-2);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: none;
    font-size: 13px;
    color: var(--fg-0);
  }
  .search input::placeholder {
    color: var(--fg-3);
  }
  .search-x {
    display: inline-flex;
    padding: 4px;
    border-radius: 6px;
    color: var(--fg-3);
  }
  .search-x:hover {
    color: var(--fg-0);
  }
  .history {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 8px 12px;
    mask-image: linear-gradient(180deg, transparent 0, #000 10px, #000 calc(100% - 16px), transparent);
  }
  .group + .group {
    margin-top: 14px;
  }
  .group-label {
    margin: 8px 10px 4px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--fg-3);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    position: relative;
    display: flex;
    align-items: center;
    border-radius: 9px;
  }
  .row:hover {
    background: var(--bg-2);
  }
  .row.active {
    background: var(--bg-3);
  }
  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 9px;
    height: 34px;
    padding: 0 10px;
    text-align: left;
    font-size: 13px;
    color: var(--fg-1);
  }
  .row.active .row-main,
  .row:hover .row-main {
    color: var(--fg-0);
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 42%;
    flex: none;
    padding: 1px 6px;
    font-size: 10.5px;
    color: var(--fg-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row:hover .tag,
  .row:focus-within .tag {
    display: none;
  }
  .compare-box {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 6px;
    margin: 2px 2px 4px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .cmp-sel,
  .cmp-in {
    width: 100%;
    font-size: 11px;
    padding: 4px 6px;
    background: var(--bg-0);
    color: var(--fg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .cmp-go {
    padding: 4px 8px;
    font-size: 11px;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-fg);
    text-align: center;
  }
  .row-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row-actions {
    display: none;
    gap: 1px;
    padding-right: 4px;
  }
  .row:hover .row-actions,
  .row:focus-within .row-actions {
    display: flex;
  }
  .row-actions button {
    display: inline-flex;
    padding: 5px;
    border-radius: 6px;
    color: var(--fg-3);
  }
  .row-actions button:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .row-actions .del:hover {
    color: var(--err);
  }
  .rename {
    flex: 1;
    height: 34px;
    padding: 0 10px;
    font-size: 13px;
    color: var(--fg-0);
    background: var(--bg-0);
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    outline: none;
  }
  .empty {
    margin: 10px;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--fg-3);
  }
  .nap {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 32px 12px;
    text-align: center;
  }
  .nap p {
    margin: 0;
    font-family: 'm5x7', var(--font-mono);
    font-size: 16px;
    line-height: 1.15;
    color: var(--fg-3);
  }
  .empty.err {
    color: var(--err);
  }
  .skel-list {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 14px 10px;
  }
  .skel {
    height: 10px;
    border-radius: 3px;
    /* a scan that steps instead of gliding: reads as pixel art, and a stepped
       animation is cheaper than a per-frame gradient for a list of bars */
    background-image: linear-gradient(
      90deg,
      var(--bg-3) 0%,
      color-mix(in srgb, var(--fg-0) 9%, var(--bg-3)) 50%,
      var(--bg-3) 100%
    );
    background-size: 220% 100%;
    animation: skel-scan 1.8s steps(14, end) infinite;
  }
  @keyframes skel-scan {
    from {
      background-position: 130% 0;
    }
    to {
      background-position: -30% 0;
    }
  }
  .foot {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px;
    border-top: 1px solid var(--border);
  }
  .who {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px 2px;
    font-size: 11.5px;
    color: var(--fg-3);
    min-width: 0;
  }
  .who-logo {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    color: var(--fg-2);
  }
  .who-text {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
    background: var(--fg-3);
  }
  .dot.on {
    background: var(--ok);
  }
</style>
