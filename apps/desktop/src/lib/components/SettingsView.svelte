<script lang="ts">
  // Settings — providers & connections, workspace folder, app facts.
  // All data comes from Tauri commands; nothing here is fixture state.
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type { AppInfo, NousLoginInfo, ProviderInfo } from '../ipc';

  let providers = $state<ProviderInfo[]>([]);
  let appInfo = $state<AppInfo | null>(null);
  let inTauri = $state(false);
  let error = $state<string | null>(null);

  // Nous device-login state (mirrors EmptyState's flow).
  let login = $state<NousLoginInfo | null>(null);
  let loginBusy = $state(false);
  let loginError = $state<string | null>(null);
  let unlisten: UnlistenFn | null = null;

  // Per-provider key-entry drafts (never persisted to disk by us —
  // they go straight to the OS keychain via auth_set_key).
  let keyDrafts = $state<Record<string, string>>({});
  let keyBusy = $state<string | null>(null);

  const THEMES = ['light', 'dark', 'auto'] as const;

  async function refresh(): Promise<void> {
    if (!inTauri) return;
    const m = await import('../ipc');
    providers = await m.providersList().catch(() => []);
    appInfo = await m.appInfo().catch(() => null);
  }

  onMount(async () => {
    try {
      const m = await import('../ipc');
      await m.appInfo();
      inTauri = true;
      await refresh();
      const { listen } = await import('@tauri-apps/api/event');
      unlisten = await listen<{ type: string; ok: boolean; error?: string }>(
        'auth-event',
        (e) => {
          if (e.payload.type === 'nous_login') {
            loginBusy = false;
            if (e.payload.ok) {
              login = null;
              void refresh();
            } else {
              loginError = e.payload.error ?? 'login failed';
            }
          }
        },
      );
    } catch {
      /* fixture mode */
    }
  });

  onDestroy(() => unlisten?.());

  async function connectNous(): Promise<void> {
    loginBusy = true;
    loginError = null;
    try {
      const m = await import('../ipc');
      login = await m.authNousLogin();
    } catch (e) {
      loginBusy = false;
      loginError = e instanceof Error ? e.message : String(e);
    }
  }

  async function cancelLogin(): Promise<void> {
    const m = await import('../ipc');
    await m.authNousCancel().catch(() => {});
    login = null;
    loginBusy = false;
  }

  async function disconnect(name: string): Promise<void> {
    const m = await import('../ipc');
    await m.authLogout(name).catch((e: unknown) => {
      error = String(e);
    });
    await refresh();
  }

  async function saveKey(name: string): Promise<void> {
    const key = keyDrafts[name]?.trim();
    if (!key) return;
    keyBusy = name;
    error = null;
    try {
      const m = await import('../ipc');
      await m.authSetKey(name, key);
      keyDrafts[name] = '';
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      keyBusy = null;
    }
  }

  function authLabel(p: ProviderInfo): string {
    if (p.auth_kind === 'oauth') return 'subscription';
    if (p.auth_kind === 'api_key') return 'api key';
    return 'not connected';
  }
</script>

<div class="settings">
  <div class="settings-inner">
    <h1 class="page-title">Settings</h1>

    <section class="group">
      <h2 class="group-title">Appearance</h2>
      <p class="group-hint">
        Light or dark. Auto follows your system's setting.
      </p>
      <div class="card">
        <div class="seg" role="group" aria-label="Theme">
          {#each THEMES as t}
            <button
              class="seg-item"
              class:active={session.theme === t}
              onclick={() => session.setTheme(t)}
              aria-pressed={session.theme === t}
            >
              {t}
            </button>
          {/each}
        </div>
      </div>
    </section>

    <section class="group">
      <h2 class="group-title">Providers</h2>
      <p class="group-hint">
        Connect a provider to chat or run code sessions. Keys go to the OS
        keychain; Portal logins use your subscription.
      </p>

      {#if !inTauri}
        <p class="muted">Provider management needs the desktop app.</p>
      {:else}
        <div class="provider-list">
          {#each providers as p (p.name)}
            <div class="provider">
              <div class="provider-head">
                <span class="provider-name">{p.name}</span>
                {#if p.is_default}
                  <span class="badge">default</span>
                {/if}
                <span class="badge" class:connected={p.auth_kind !== 'none'}>
                  {authLabel(p)}
                </span>
              </div>
              <div class="provider-meta mono">
                {p.kind} · {p.model}
              </div>
              <div class="provider-url mono" title={p.base_url}>{p.base_url}</div>

              {#if p.name === 'nous'}
                {#if login}
                  <div class="login-box">
                    <p class="login-line">
                      Approve at
                      <a href={login.verification_url} target="_blank" rel="noreferrer">
                        {login.verification_url}
                      </a>
                      — code <code>{login.user_code}</code>
                    </p>
                    <button class="btn ghost" onclick={cancelLogin}>Cancel</button>
                  </div>
                {:else}
                  <div class="provider-actions">
                    <button
                      class="btn primary"
                      onclick={connectNous}
                      disabled={loginBusy || p.auth_kind !== 'none'}
                    >
                      {p.auth_kind === 'none' ? 'Connect Nous Portal' : 'Reconnect'}
                    </button>
                    {#if p.auth_kind !== 'none'}
                      <button class="btn" onclick={() => disconnect(p.name)}>Disconnect</button>
                    {/if}
                  </div>
                {/if}
                {#if loginError}
                  <p class="error-text">{loginError}</p>
                {/if}
              {:else}
                <div class="provider-actions">
                  <input
                    class="key-input"
                    type="password"
                    placeholder={p.auth_kind === 'api_key' ? 'replace key…' : 'API key…'}
                    bind:value={keyDrafts[p.name]}
                  />
                  <button
                    class="btn"
                    onclick={() => saveKey(p.name)}
                    disabled={!keyDrafts[p.name]?.trim() || keyBusy === p.name}
                  >
                    {keyBusy === p.name ? 'Saving…' : 'Save key'}
                  </button>
                  {#if p.auth_kind !== 'none'}
                    <button class="btn ghost" onclick={() => disconnect(p.name)}>Disconnect</button>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
      {#if error}
        <p class="error-text">{error}</p>
      {/if}
    </section>

    <section class="group">
      <h2 class="group-title">Workspace</h2>
      <p class="group-hint">
        Folder code sessions operate in. Chat doesn't use it.
      </p>
      <div class="workspace-row">
        <Icon name="folder" size={14} />
        <span class="workspace-path mono">
          {session.workspacePath ?? 'No folder — uses app data directory'}
        </span>
        <span class="spacer"></span>
        <button class="btn" onclick={() => session.pickWorkspace()}>
          Choose folder…
        </button>
        {#if session.workspacePath}
          <button
            class="btn ghost"
            onclick={async () => {
              const m = await import('../ipc');
              await m.workspaceSet(null).catch(() => {});
              session.workspacePath = null;
            }}
          >
            Clear
          </button>
        {/if}
      </div>
    </section>

    <section class="group">
      <h2 class="group-title">MCP servers</h2>
      <p class="group-hint muted">
        Tool servers (Model Context Protocol) are planned for a later
        milestone — nothing to configure yet.
      </p>
    </section>

    <section class="group">
      <h2 class="group-title">About</h2>
      <div class="card">
        {#if appInfo}
          <dl class="about-grid">
            <dt>Version</dt><dd class="mono">{appInfo.version}</dd>
            <dt>Platform</dt><dd class="mono">{appInfo.platform} · {appInfo.arch}</dd>
            <dt>Data</dt><dd class="mono about-path" title={appInfo.data_dir}>{appInfo.data_dir}</dd>
          </dl>
        {:else}
          <p class="muted">App info unavailable outside the desktop app.</p>
        {/if}
      </div>
    </section>
  </div>
</div>

<style>
  .settings {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    background: var(--bg-0);
  }
  .settings-inner {
    max-width: 680px;
    margin: 0 auto;
    padding: 28px 24px 48px;
  }
  .page-title {
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    color: var(--fg-0);
    margin: 0 0 22px;
  }
  .group {
    margin-bottom: 30px;
  }
  .card {
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 14px 16px;
    box-shadow: var(--shadow-sm);
  }
  .seg {
    display: inline-flex;
    gap: 4px;
    padding: 3px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .seg-item {
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    padding: 6px 18px;
    border-radius: 7px;
    color: var(--fg-2);
    text-transform: capitalize;
    transition:
      background var(--motion),
      color var(--motion);
  }
  .seg-item:hover {
    color: var(--fg-0);
  }
  .seg-item.active {
    background: var(--accent);
    color: var(--accent-fg);
    box-shadow: var(--shadow-sm);
  }
  .group-title {
    font-size: 13px;
    font-weight: 700;
    color: var(--fg-0);
    margin: 0 0 4px;
  }
  .group-hint {
    font-size: 12px;
    color: var(--fg-2);
    margin: 0 0 12px;
    line-height: 1.5;
  }
  .provider-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .provider {
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 14px 16px;
    box-shadow: var(--shadow-sm);
  }
  .provider-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }
  .provider-name {
    font-size: 14px;
    font-weight: 700;
    color: var(--fg-0);
  }
  .badge {
    font-size: 10px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: var(--radius-pill);
    background: var(--bg-3);
    color: var(--fg-2);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .badge.connected {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .provider-meta {
    font-size: 11.5px;
    color: var(--fg-1);
  }
  .provider-url {
    font-size: 10.5px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-bottom: 10px;
  }
  .provider-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .key-input {
    flex: 1;
    min-width: 160px;
    padding: 6px 10px;
    font-size: 12px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--fg-0);
  }
  .key-input:focus {
    outline: none;
    border-color: var(--accent-dim);
  }
  .login-box {
    padding: 8px 0 2px;
  }
  .login-line {
    font-size: 12px;
    color: var(--fg-1);
    margin: 0 0 8px;
    line-height: 1.6;
  }
  .login-line a {
    color: var(--fg-0);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .login-line code {
    font-family: var(--font-mono);
    background: var(--bg-2);
    padding: 1px 6px;
    border-radius: var(--radius-sm);
  }
  .workspace-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    color: var(--fg-2);
  }
  .workspace-path {
    font-size: 12px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .btn {
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    padding: 6px 12px;
    border-radius: var(--radius);
    border: 1px solid var(--border-strong);
    background: var(--bg-2);
    color: var(--fg-0);
    cursor: pointer;
    transition:
      background var(--motion),
      border-color var(--motion);
  }
  .btn:hover {
    background: var(--bg-3);
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
  }
  .btn.primary:hover {
    background: var(--accent);
    filter: brightness(1.08);
  }
  .btn.ghost {
    background: transparent;
    border-color: var(--border);
    color: var(--fg-2);
  }
  .btn.ghost:hover {
    color: var(--fg-0);
    background: var(--bg-2);
  }
  .error-text {
    font-size: 12px;
    color: var(--err);
    margin: 8px 0 0;
  }
  .muted {
    font-size: 12px;
    color: var(--fg-3);
  }
  .about-grid {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 8px 14px;
    margin: 0;
    font-size: 12px;
  }
  .about-grid dt {
    color: var(--fg-3);
  }
  .about-grid dd {
    margin: 0;
    color: var(--fg-1);
  }
  .about-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
