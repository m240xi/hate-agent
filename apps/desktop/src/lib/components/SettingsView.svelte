<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type {
    AppInfo,
    ClaudeLoginInfo,
    McpServerConfig,
    McpServerInfo,
    NousLoginInfo,
    ProviderInfo,
    ProviderPreset,
    SkillInfo,
  } from '../ipc';

  let providers = $state<ProviderInfo[]>([]);
  let presets = $state<ProviderPreset[]>([]);
  let skills = $state<SkillInfo[]>([]);
  let skillAuto = $state(true);
  let skillOpen = $state<string | null>(null);
  let skillBodies = $state<Record<string, string>>({});
  let appInfo = $state<AppInfo | null>(null);
  let inTauri = $state(false);
  let error = $state<string | null>(null);

  let login = $state<NousLoginInfo | null>(null);
  let loginBusy = $state(false);
  let loginError = $state<string | null>(null);
  let unlisten: UnlistenFn | null = null;

  let claudeLogin = $state<ClaudeLoginInfo | null>(null);
  let claudeCode = $state('');
  let claudeBusy = $state(false);
  let claudeError = $state<string | null>(null);
  let presetBusy = $state<string | null>(null);

  let keyDrafts = $state<Record<string, string>>({});
  let keyBusy = $state<string | null>(null);

  import { THEMES } from '../themes';

  async function refresh(): Promise<void> {
    if (!inTauri) return;
    const m = await import('../ipc');
    providers = await m.providersList().catch(() => []);
    presets = await m.providerPresets().catch(() => []);
    skills = await m.skillsList().catch(() => []);
    skillAuto = await m.skillsAutoGet().catch(() => true);
    appInfo = await m.appInfo().catch(() => null);
    await loadMcp();
    void session.refreshModels();
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
          } else if (e.payload.type === 'claude_login') {
            claudeBusy = false;
            if (e.payload.ok) {
              claudeLogin = null;
              claudeCode = '';
              void refresh();
            } else {
              claudeError = e.payload.error ?? 'login failed';
            }
          }
        },
      );
    } catch {
    }
  });

  onDestroy(() => {
    unlisten?.();
    if (mcpSaveTimer) {
      clearTimeout(mcpSaveTimer);
      mcpSaveTimer = null;
    }
    if (mcpDirty && inTauri) void flushMcpSave();
    if (mcpPoll) {
      clearInterval(mcpPoll);
      mcpPoll = null;
    }
  });

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

  const addablePresets = $derived(
    presets.filter((p) => !providers.some((q) => q.name === p.name)),
  );

  async function addPreset(preset: ProviderPreset): Promise<void> {
    presetBusy = preset.name;
    error = null;
    try {
      const m = await import('../ipc');
      await m.providerUpsert(preset.name, preset.kind, preset.base_url, preset.model);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      presetBusy = null;
    }
  }

  async function removeProvider(name: string): Promise<void> {
    const m = await import('../ipc');
    await m.providerRemove(name).catch((e: unknown) => {
      error = String(e);
    });
    await refresh();
  }

  async function connectClaude(): Promise<void> {
    claudeBusy = true;
    claudeError = null;
    try {
      const m = await import('../ipc');
      claudeLogin = await m.authClaudeBegin();
    } catch (e) {
      claudeBusy = false;
      claudeError = e instanceof Error ? e.message : String(e);
    }
  }

  async function toggleSkill(name: string): Promise<void> {
    if (skillOpen === name) {
      skillOpen = null;
      return;
    }
    skillOpen = name;
    if (!skillBodies[name]) {
      const m = await import('../ipc');
      const body = await m.skillsRead(name).catch(() => '(unreadable)');
      skillBodies = { ...skillBodies, [name]: body };
    }
  }

  async function removeSkill(name: string): Promise<void> {
    const m = await import('../ipc');
    await m.skillsDelete(name).catch((e: unknown) => {
      error = String(e);
    });
    skillOpen = null;
    await refresh();
  }

  async function setAutoSkills(enabled: boolean): Promise<void> {
    skillAuto = enabled;
    const m = await import('../ipc');
    await m.skillsSetAuto(enabled).catch((e: unknown) => {
      error = String(e);
    });
  }

  async function finishClaudeLogin(): Promise<void> {
    const code = claudeCode.trim();
    if (!code) return;
    claudeBusy = true;
    claudeError = null;
    try {
      const m = await import('../ipc');
      await m.authClaudeComplete(code);
      claudeCode = '';
    } catch (e) {
      claudeBusy = false;
      claudeError = e instanceof Error ? e.message : String(e);
    }
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

  let mcpRows = $state<McpServerInfo[]>([]);
  let mcpDirty = $state(false);
  let mcpBusy = $state(false);
  let mcpError = $state<string | null>(null);
  let mcpLoaded = $state(false);
  let mcpOpen = $state<number | null>(null);
  let showMcpAdd = $state(false);

  let mcpSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let mcpPoll: ReturnType<typeof setInterval> | null = null;
  let mcpSaving = false;
  let mcpSavePending = false;

  let nfName = $state('');
  let nfKind = $state<'stdio' | 'http'>('stdio');
  let nfCommand = $state('');
  let nfArgs = $state('');
  let nfEnv = $state('');
  let nfUrl = $state('');
  let nfHeaders = $state('');

  let editEnv = $state('');
  let editHeaders = $state('');

  function splitArgs(text: string): string[] {
    const out: string[] = [];
    const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(text)) !== null) {
      if (m[1] !== undefined || m[2] !== undefined) out.push(m[1] ?? m[2] ?? '');
      else if (m[3]) out.push(m[3]);
    }
    return out;
  }

  function pairsToText(rec: Record<string, string>): string {
    return Object.entries(rec)
      .map(([k, v]) => `${k}=${v}`)
      .join('\n');
  }

  function textToPairs(text: string): Record<string, string> {
    const out: Record<string, string> = {};
    for (const line of text.split('\n')) {
      const t = line.trim();
      if (!t) continue;
      const eq = t.indexOf('=');
      if (eq > 0) out[t.slice(0, eq).trim()] = t.slice(eq + 1).trim();
    }
    return out;
  }

  async function loadMcp(): Promise<void> {
    if (!inTauri || mcpLoaded) return;
    const m = await import('../ipc');
    mcpRows = await m.mcpList().catch(() => []);
    mcpLoaded = true;
    if (!mcpPoll) mcpPoll = setInterval(() => void pollMcp(), 8000);
  }

  async function pollMcp(): Promise<void> {
    if (!inTauri || mcpDirty || mcpBusy) return;
    const m = await import('../ipc');
    const rows = await m.mcpList().catch(() => null);
    if (!rows) return;
    mcpRows = mcpRows.map((row) => {
      const live = rows.find((r) => r.name === row.name);
      return live ? { ...row, state: live.state, tools: live.tools, info: live.info } : row;
    });
  }

  async function refreshMcp(): Promise<void> {
    if (!inTauri) return;
    mcpBusy = true;
    try {
      const m = await import('../ipc');
      mcpRows = await m.mcpRefresh();
    } finally {
      mcpBusy = false;
    }
  }

  function openMcpEditor(i: number): void {
    mcpOpen = mcpOpen === i ? null : i;
    const row = mcpRows[i];
    if (row && mcpOpen === i) {
      editEnv = pairsToText(row.env ?? {});
      editHeaders = pairsToText(row.headers ?? {});
    }
  }

  function addMcpRow(): void {
    const name = nfName.trim();
    if (!name) {
      mcpError = 'server needs a name';
      return;
    }
    if (nfKind === 'stdio' && !nfCommand.trim()) {
      mcpError = 'stdio server needs a command (e.g. npx)';
      return;
    }
    if (nfKind === 'http' && !nfUrl.trim()) {
      mcpError = 'http server needs a URL';
      return;
    }
    if (mcpRows.some((r) => r.name.toLowerCase() === name.toLowerCase())) {
      mcpError = `duplicate name "${name}"`;
      return;
    }
    const row: McpServerInfo = {
      name,
      kind: nfKind,
      enabled: true,
      state: 'new',
      tools: 0,
      info: '',
      command: nfKind === 'stdio' ? nfCommand.trim() : null,
      args: nfKind === 'stdio' ? splitArgs(nfArgs) : [],
      env: nfKind === 'stdio' ? textToPairs(nfEnv) : {},
      url: nfKind === 'http' ? nfUrl.trim() : null,
      headers: nfKind === 'http' ? textToPairs(nfHeaders) : {},
    };
    mcpRows = [...mcpRows, row];
    queueMcpSave();
    mcpError = null;
    nfName = nfCommand = nfArgs = nfEnv = nfUrl = nfHeaders = '';
    showMcpAdd = false;
  }

  function removeMcpRow(i: number): void {
    mcpRows = mcpRows.filter((_, j) => j !== i);
    if (mcpOpen === i) mcpOpen = null;
    else if (mcpOpen !== null && mcpOpen > i) mcpOpen -= 1;
    queueMcpSave();
  }

  function mcpKindSwitch(row: McpServerInfo, kind: 'stdio' | 'http'): void {
    row.kind = kind;
    if (kind === 'stdio') {
      row.url = null;
      row.headers = {};
    } else {
      row.command = null;
      row.args = [];
      row.env = {};
    }
    queueMcpSave();
  }

  function queueMcpSave(): void {
    mcpDirty = true;
    if (mcpSaveTimer) clearTimeout(mcpSaveTimer);
    mcpSaveTimer = setTimeout(() => void flushMcpSave(), 800);
  }

  async function flushMcpSave(): Promise<void> {
    if (mcpSaveTimer) {
      clearTimeout(mcpSaveTimer);
      mcpSaveTimer = null;
    }
    if (mcpSaving) {
      mcpSavePending = true;
      return;
    }
    if (!inTauri) return;
    const m = await import('../ipc');
    mcpSaving = true;
    mcpBusy = true;
    mcpError = null;
    try {
      const configs: McpServerConfig[] = mcpRows.map((r) => ({
        name: r.name,
        kind: r.kind as 'stdio' | 'http',
        enabled: r.enabled,
        command: r.command ?? null,
        args: r.args ?? [],
        env: r.env ?? {},
        url: r.url ?? null,
        headers: r.headers ?? {},
      }));
      const saved = await m.mcpSave(configs);
      const byName = new Map(saved.map((s) => [s.name.toLowerCase(), s]));
      mcpRows = mcpRows.map((r) => {
        const s = byName.get(r.name.toLowerCase());
        return s ? { ...r, state: s.state, tools: s.tools, info: s.info } : r;
      });
      mcpDirty = mcpSavePending;
    } catch (e) {
      mcpError = e instanceof Error ? e.message : String(e);
    } finally {
      mcpSaving = false;
      mcpBusy = false;
      if (mcpSavePending) {
        mcpSavePending = false;
        void flushMcpSave();
      }
    }
  }

  function mcpStateLabel(row: McpServerInfo): string {
    if (row.state === 'connected') return `connected · ${row.tools} tool${row.tools === 1 ? '' : 's'}`;
    if (row.state === 'connecting' || row.state === 'new') return 'saving…';
    if (row.state === 'disabled') return 'disabled';
    return 'error';
  }
</script>

<div class="settings">
  <div class="settings-inner">
    <h1 class="page-title">Settings</h1>

    <section class="group">
      <h2 class="group-title">Appearance</h2>
      <p class="group-hint">
        Pick a palette — Auto follows your system's setting.
      </p>
      <div class="card">
        <div class="theme-grid" role="group" aria-label="Theme">
          {#each THEMES as t}
            <button
              class="theme-card"
              class:active={session.theme === t.id}
              onclick={() => session.setTheme(t.id)}
              aria-pressed={session.theme === t.id}
              title={t.hint}
            >
              <span class="theme-prev" style:background={t.bg}>
                <span class="theme-bar" style:background={t.accent}></span>
                <span class="theme-line" style:background={t.fg}></span>
              </span>
              <span class="theme-name">{t.label}</span>
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
                {#if p.name === 'claude'}
                  {#if claudeLogin}
                    <div class="login-box">
                      <p class="login-line">
                        Approve at
                        <a
                          href={claudeLogin.authorize_url}
                          target="_blank"
                          rel="noreferrer"
                        >
                          {claudeLogin.authorize_url}
                        </a>
                      </p>
                      <div class="provider-actions">
                        <input
                          class="key-input"
                          type="text"
                          placeholder="paste the code shown…"
                          bind:value={claudeCode}
                          onkeydown={(e) => e.key === 'Enter' && finishClaudeLogin()}
                        />
                        <button
                          class="btn primary"
                          onclick={finishClaudeLogin}
                          disabled={!claudeCode.trim() || claudeBusy}
                        >
                          {claudeBusy ? 'Signing in…' : 'Finish sign-in'}
                        </button>
                        <button
                          class="btn ghost"
                          onclick={() => {
                            claudeLogin = null;
                            claudeCode = '';
                            claudeError = null;
                          }}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  {:else}
                    <div class="provider-actions">
                      <button
                        class="btn primary"
                        onclick={connectClaude}
                        disabled={claudeBusy}
                      >
                        {p.auth_kind === 'oauth'
                          ? 'Reconnect Claude'
                          : 'Sign in with Claude'}
                      </button>
                      <span class="provider-hint">Pro / Max subscription</span>
                    </div>
                  {/if}
                  {#if claudeError}
                    <p class="error-text">{claudeError}</p>
                  {/if}
                {/if}
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
                  {#if !p.is_default}
                    <button class="btn ghost" onclick={() => removeProvider(p.name)}>Remove</button>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}

          {#if addablePresets.length > 0}
            <p class="preset-title">Add a provider</p>
            <div class="preset-grid">
              {#each addablePresets as preset}
                <button
                  class="preset-card"
                  onclick={() => addPreset(preset)}
                  disabled={presetBusy === preset.name}
                  title={preset.key_url ?? preset.base_url}
                >
                  <span class="preset-name">{preset.label}</span>
                  <span class="preset-meta">
                    {preset.auth === 'none'
                      ? 'local · no key'
                      : preset.auth === 'oauth'
                        ? 'subscription or key'
                        : 'api key'}
                  </span>
                </button>
              {/each}
            </div>
          {/if}
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
      <h2 class="group-title">Skills</h2>
      <p class="group-hint">
        Markdown knowledge files the agent writes from your sessions — best
        practices, commands, and pitfalls it reuses automatically. Matched
        skills ride the prompt; everything else is one
        <code class="mono">skill_view</code> call away.
      </p>
      <div class="card">
        <label class="skill-toggle">
          <input
            type="checkbox"
            checked={skillAuto}
            onchange={(e) => setAutoSkills(e.currentTarget.checked)}
          />
          <span>Auto-create skills from sessions (one small model call after 3+ turns)</span>
        </label>
        {#if skills.length === 0}
          <p class="muted skill-empty">No skills yet — they appear here as you work.</p>
        {:else}
          <div class="skill-list">
            {#each skills as s (s.name)}
              <div class="skill-row">
                <button class="skill-head" onclick={() => toggleSkill(s.name)}>
                  <span class="skill-title">
                    {skillOpen === s.name ? '▾' : '▸'} {s.title}
                  </span>
                  <span class="skill-meta mono">
                    {s.name} · {(s.bytes / 1024).toFixed(1)} KB
                    {#if s.triggers.length > 0}
                      · {s.triggers.join(', ')}
                    {/if}
                  </span>
                </button>
                <button class="btn ghost" onclick={() => removeSkill(s.name)}>Delete</button>
              </div>
              {#if skillOpen === s.name}
                <pre class="skill-body mono">{skillBodies[s.name] ?? 'loading…'}</pre>
              {/if}
            {/each}
          </div>
        {/if}
      </div>
    </section>

    <section class="group">
      <h2 class="group-title">
        MCP servers
        <button
          class="btn ghost mcp-refresh"
          onclick={() => void refreshMcp()}
          disabled={mcpBusy}
          title="Reconnect servers that aren't up yet — e.g. an app that started after hate-agent"
        >{mcpBusy ? 'reconnecting…' : 'refresh'}</button>
      </h2>
      <p class="group-hint muted">
        Model Context Protocol tools — stdio commands and HTTP endpoints.
        Their tools reach the agent as <code class="mono">mcp__server__tool</code>;
        anything not marked read-only asks permission, same as bash.
      </p>

      {#if mcpRows.length > 0}
        <div class="card mcp-list">
          {#each mcpRows as row, i (row.name)}
            <div class="mcp-row" class:mcp-off={!row.enabled}>
              <div class="mcp-head">
                <span
                  class="mcp-dot"
                  class:dot-ok={row.state === 'connected'}
                  class:dot-busy={row.state === 'connecting' || row.state === 'new'}
                  class:dot-err={row.state === 'error'}
                  title={row.state}
                ></span>
                <span class="mcp-name mono">{row.name}</span>
                <span class="mcp-kind">{row.kind}</span>
                <span
                  class="mcp-state"
                  class:state-err={row.state === 'error'}
                  class:state-ok={row.state === 'connected'}
                >{mcpStateLabel(row)}</span>
                <span class="mcp-spacer"></span>
                <label class="mcp-enable" title="Enabled servers connect at startup and after save">
                  <input
                    type="checkbox"
                    bind:checked={row.enabled}
                    onchange={queueMcpSave}
                  />
                  on
                </label>
                <button
                  class="icon-btn"
                  title={mcpOpen === i ? 'Close editor' : 'Edit server'}
                  onclick={() => openMcpEditor(i)}
                >
                  <Icon name="edit" size={13} />
                </button>
                <button
                  class="icon-btn danger"
                  title="Remove server"
                  onclick={() => removeMcpRow(i)}
                >
                  <Icon name="trash" size={13} />
                </button>
              </div>
              <div class="mcp-sub muted mono">
                {#if row.kind === 'stdio'}
                  {[row.command, ...(row.args ?? [])].filter(Boolean).join(' ')}
                {:else}
                  {row.url}
                {/if}
                {#if row.info}<span class="mcp-info"> · {row.info}</span>{/if}
              </div>

              {#if mcpOpen === i}
                <div class="mcp-editor">
                  <label class="fld">
                    <span class="fld-label">Name</span>
                    <input class="input" bind:value={row.name} oninput={queueMcpSave} />
                  </label>
                  <div class="fld">
                    <span class="fld-label">Transport</span>
                    <div class="seg seg-sm" role="group">
                      <button
                        class="seg-item"
                        class:active={row.kind === 'stdio'}
                        onclick={() => mcpKindSwitch(row, 'stdio')}
                      >stdio</button>
                      <button
                        class="seg-item"
                        class:active={row.kind === 'http'}
                        onclick={() => mcpKindSwitch(row, 'http')}
                      >http</button>
                    </div>
                  </div>
                  {#if row.kind === 'stdio'}
                    <label class="fld">
                      <span class="fld-label">Command</span>
                      <input
                        class="input mono"
                        placeholder="npx"
                        bind:value={row.command}
                        oninput={queueMcpSave}
                      />
                    </label>
                    <label class="fld">
                      <span class="fld-label">Args (space-separated, "…" for spaces)</span>
                      <input
                        class="input mono"
                        placeholder="-y @modelcontextprotocol/server-filesystem ."
                        value={(row.args ?? []).map((a) => (/\s/.test(a) ? `"${a}"` : a)).join(' ')}
                        oninput={(e) => {
                          row.args = splitArgs((e.currentTarget as HTMLInputElement).value);
                          queueMcpSave();
                        }}
                      />
                    </label>
                    <label class="fld">
                      <span class="fld-label">Env (KEY=VALUE per line)</span>
                      <textarea
                        class="input mono"
                        rows="2"
                        bind:value={editEnv}
                        oninput={() => {
                          row.env = textToPairs(editEnv);
                          queueMcpSave();
                        }}
                      ></textarea>
                    </label>
                  {:else}
                    <label class="fld">
                      <span class="fld-label">URL</span>
                      <input
                        class="input mono"
                        placeholder="http://localhost:3002/mcp"
                        bind:value={row.url}
                        oninput={queueMcpSave}
                      />
                    </label>
                    <label class="fld">
                      <span class="fld-label">Headers (Name: value per line)</span>
                      <textarea
                        class="input mono"
                        rows="2"
                        placeholder={"Authorization: Bearer …"}
                        bind:value={editHeaders}
                        oninput={() => {
                          row.headers = Object.fromEntries(
                            Object.entries(textToPairs(editHeaders)).map(([k, v]) => [
                              k.replace(/:$/, ''),
                              v,
                            ]),
                          );
                          queueMcpSave();
                        }}
                      ></textarea>
                    </label>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {:else if mcpLoaded}
        <div class="card">
          <p class="muted mcp-empty">
            No MCP servers yet — add one below and its tools show up in
            every session, Chat included.
          </p>
        </div>
      {/if}

      {#if showMcpAdd}
        <div class="card mcp-add">
          <div class="mcp-add-grid">
            <label class="fld">
              <span class="fld-label">Name</span>
              <input class="input" placeholder="roblox" bind:value={nfName} />
            </label>
            <div class="fld">
              <span class="fld-label">Transport</span>
              <div class="seg seg-sm" role="group">
                <button
                  class="seg-item"
                  class:active={nfKind === 'stdio'}
                  onclick={() => (nfKind = 'stdio')}
                >stdio</button>
                <button
                  class="seg-item"
                  class:active={nfKind === 'http'}
                  onclick={() => (nfKind = 'http')}
                >http</button>
              </div>
            </div>
            {#if nfKind === 'stdio'}
              <label class="fld">
                <span class="fld-label">Command</span>
                <input class="input mono" placeholder="npx" bind:value={nfCommand} />
              </label>
              <label class="fld">
                <span class="fld-label">Args (space-separated, "…" for spaces)</span>
                <input
                  class="input mono"
                  placeholder="-y @modelcontextprotocol/server-filesystem ."
                  bind:value={nfArgs}
                />
              </label>
              <label class="fld fld-wide">
                <span class="fld-label">Env (KEY=VALUE per line)</span>
                <textarea class="input mono" rows="2" bind:value={nfEnv}></textarea>
              </label>
            {:else}
              <label class="fld fld-wide">
                <span class="fld-label">URL</span>
                <input
                  class="input mono"
                  placeholder="http://localhost:3002/mcp"
                  bind:value={nfUrl}
                />
              </label>
              <label class="fld fld-wide">
                <span class="fld-label">Headers (Name: value per line)</span>
                <textarea
                  class="input mono"
                  rows="2"
                  placeholder={"Authorization: Bearer …"}
                  bind:value={nfHeaders}
                ></textarea>
              </label>
            {/if}
          </div>
          <div class="mcp-add-actions">
            <button class="btn primary" onclick={addMcpRow}>Add server</button>
            <button class="btn ghost" onclick={() => (showMcpAdd = false)}>Cancel</button>
          </div>
        </div>
      {:else}
        <button class="btn ghost mcp-add-btn" onclick={() => (showMcpAdd = true)}>
          <Icon name="plus" size={13} /> Add MCP server
        </button>
      {/if}

      {#if mcpDirty || mcpError || mcpBusy}
        <div class="mcp-foot">
          {#if mcpError}<span class="mcp-error">{mcpError}</span>{/if}
          <button class="btn primary" disabled={mcpBusy} onclick={() => void flushMcpSave()}>
            {mcpBusy ? 'Saving…' : 'Save now'}
          </button>
          {#if !mcpDirty && !mcpBusy && !mcpError}<span class="muted">changes save automatically</span>{/if}
        </div>
      {/if}
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
    padding: 30px 24px 72px;
  }
  .page-title {
    font-family: var(--font-ui);
    font-size: 38px;
    font-weight: 400;
    color: var(--fg-0);
    margin: 0 0 30px;
  }
  .group {
    margin-bottom: 34px;
  }
  .group-title {
    font-family: var(--font-mono);
    font-size: 16px;
    color: var(--fg-0);
    margin: 0 0 4px;
  }
  .group-hint {
    font-family: var(--font-mono);
    font-size: 14px;
    color: var(--fg-2);
    margin: 0 0 12px;
    line-height: 1.6;
  }

  .card,
  .provider,
  .workspace-row {
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-sm);
  }
  .card {
    padding: 14px 16px;
  }

  .provider-name,
  .provider-meta,
  .provider-url,
  .workspace-path,
  .about-grid dd,
  .login-line,
  .login-line code,
  .key-input {
    font-family: var(--font-mono);
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
    font-family: var(--font-mono);
    font-size: 14px;
    padding: 7px 18px;
    border-radius: 5px;
    border: 1px solid transparent;
    color: var(--fg-2);
    text-transform: capitalize;
    transition:
      background var(--motion),
      color var(--motion),
      border-color var(--motion);
  }
  .seg-item:hover {
    color: var(--fg-0);
  }
  .seg-item.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
    box-shadow: 0 2px 0 rgba(0, 0, 0, 0.4);
  }
  .theme-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(108px, 1fr));
    gap: 10px;
  }
  .theme-card {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 8px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    text-align: left;
    transition:
      border-color var(--motion),
      transform var(--motion);
  }
  .theme-card:hover {
    border-color: var(--border-strong);
    transform: translateY(-1px);
  }
  .theme-card.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .theme-prev {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: 5px;
    height: 44px;
    padding: 8px;
    border-radius: 5px;
    border: 1px solid rgba(128, 128, 128, 0.25);
    overflow: hidden;
  }
  .theme-bar {
    height: 5px;
    width: 55%;
    border-radius: 2px;
  }
  .theme-line {
    height: 3px;
    width: 80%;
    border-radius: 2px;
    opacity: 0.55;
  }
  .theme-name {
    font-family: var(--font-ui);
    font-size: 12px;
    color: var(--fg-1);
  }
  .theme-card.active .theme-name {
    color: var(--accent);
  }
  .provider-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .provider {
    padding: 14px 16px;
  }
  .provider-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }
  .provider-name {
    font-size: 16px;
    color: var(--fg-0);
  }
  .badge {
    font-family: var(--font-mono);
    font-size: 12px;
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
    font-size: 14px;
    color: var(--fg-1);
  }
  .provider-url {
    font-size: 13px;
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
    padding: 8px 12px;
    font-size: 14px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--fg-0);
  }
  .key-input::placeholder {
    color: var(--fg-3);
  }
  .key-input:focus {
    outline: none;
    border-color: var(--accent-dim);
  }
  .login-box {
    padding: 8px 0 2px;
  }
  .login-line {
    font-size: 14px;
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
    background: var(--bg-2);
    padding: 1px 6px;
    border-radius: var(--radius-sm);
  }
  .provider-hint {
    font-size: 10px;
    color: var(--fg-3);
    align-self: center;
  }
  .preset-title {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-3);
    margin: 14px 0 8px;
  }
  .preset-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 8px;
  }
  .preset-card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 12px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    text-align: left;
    transition:
      border-color var(--motion),
      transform var(--motion);
  }
  .preset-card:hover:not(:disabled) {
    border-color: var(--border-strong);
    transform: translateY(-1px);
  }
  .preset-card:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .preset-name {
    font-size: 12px;
    color: var(--fg-0);
  }
  .preset-meta {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--fg-3);
  }
  .skill-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    font-size: 12px;
    color: var(--fg-1);
    cursor: pointer;
  }
  .skill-toggle input {
    accent-color: var(--accent);
  }
  .skill-empty {
    padding: 0 14px 12px;
    font-size: 12px;
  }
  .skill-list {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
  }
  .skill-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px 6px 14px;
    border-bottom: 1px solid var(--border);
  }
  .skill-row:last-of-type {
    border-bottom: none;
  }
  .skill-head {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    text-align: left;
    background: none;
    border: none;
    padding: 4px 0;
    cursor: pointer;
  }
  .skill-title {
    font-size: 12px;
    color: var(--fg-0);
  }
  .skill-meta {
    font-size: 10px;
    color: var(--fg-3);
  }
  .skill-body {
    margin: 0;
    padding: 10px 14px;
    font-size: 11px;
    line-height: 1.55;
    color: var(--fg-1);
    background: var(--bg-1);
    border-bottom: 1px solid var(--border);
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
  }
  .workspace-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    color: var(--fg-2);
  }
  .workspace-path {
    font-size: 14px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }

  .btn {
    font-family: var(--font-mono);
    font-size: 15px;
    padding: 8px 16px;
    border-radius: 5px;
    border: 2px solid #2b2d31;
    background: linear-gradient(#1a1c1f, #121315);
    color: #e9e7e1;
    text-shadow: 0 1px 0 rgba(0, 0, 0, 0.6);
    cursor: pointer;
    transition:
      transform var(--motion),
      border-color var(--motion),
      box-shadow var(--motion),
      filter var(--motion);
    box-shadow:
      inset 0 2px 0 rgba(255, 255, 255, 0.07),
      inset 0 -2px 0 rgba(0, 0, 0, 0.55),
      0 3px 0 #050505,
      var(--shadow-sm);
  }
  .btn:hover {
    border-color: var(--accent-dim);
    transform: translateY(-1px);
  }
  .btn:active {
    transform: translateY(2px);
    box-shadow:
      inset 0 2px 0 rgba(255, 255, 255, 0.05),
      inset 0 -1px 0 rgba(0, 0, 0, 0.55),
      0 1px 0 #050505,
      var(--shadow-sm);
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
    transform: none;
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
    text-shadow: none;
  }
  .btn.primary:hover {
    filter: brightness(1.06);
  }
  .btn.ghost {
    background: rgba(18, 19, 21, 0.7);
    border-color: #26282c;
    color: #b9b6ae;
  }
  .error-text {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--err);
    margin: 8px 0 0;
  }
  .muted {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--fg-3);
  }
  .about-grid {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 8px 14px;
    margin: 0;
    font-size: 13px;
  }
  .about-grid dt {
    font-family: var(--font-mono);
    color: var(--fg-3);
  }
  .about-grid dd {
    margin: 0;
    font-size: 14px;
    color: var(--fg-1);
  }
  .about-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mcp-list {
    display: flex;
    flex-direction: column;
    padding: 0;
    overflow: hidden;
  }
  .mcp-row {
    padding: 10px 14px;
    border-bottom: 1px solid rgba(38, 40, 44, 0.6);
  }
  .mcp-row:last-child {
    border-bottom: none;
  }
  .mcp-row.mcp-off {
    opacity: 0.55;
  }
  .mcp-refresh {
    font-size: 11px;
    padding: 2px 8px;
    margin-left: 8px;
    vertical-align: middle;
  }
  .mcp-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .mcp-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--fg-3);
    flex: none;
  }
  .mcp-dot.dot-ok {
    background: #4caf7d;
  }
  .mcp-dot.dot-busy {
    background: #d9a441;
    animation: mcp-pulse 1s ease-in-out infinite;
  }
  .mcp-dot.dot-err {
    background: var(--err, #d95f4e);
  }
  @keyframes mcp-pulse {
    50% {
      opacity: 0.35;
    }
  }
  .mcp-name {
    font-size: 13px;
    color: var(--fg-1);
  }
  .mcp-kind {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--fg-3);
    border: 1px solid #26282c;
    border-radius: 3px;
    padding: 1px 5px;
  }
  .mcp-state {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--fg-3);
  }
  .mcp-state.state-ok {
    color: #4caf7d;
  }
  .mcp-state.state-err {
    color: var(--err, #d95f4e);
  }
  .mcp-spacer {
    flex: 1;
  }
  .mcp-enable {
    display: flex;
    align-items: center;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--fg-3);
    cursor: pointer;
    user-select: none;
  }
  .mcp-sub {
    margin: 5px 0 0 16px;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mcp-info {
    color: var(--fg-3);
  }
  .mcp-editor {
    margin: 10px 0 2px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 520px;
  }
  .mcp-add {
    margin-top: 10px;
  }
  .mcp-add-grid {
    display: grid;
    grid-template-columns: 1fr 130px;
    gap: 10px 14px;
    padding: 14px;
  }
  .mcp-add-actions {
    display: flex;
    gap: 8px;
    padding: 0 14px 14px;
  }
  .mcp-add-btn {
    margin-top: 10px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .mcp-foot {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 10px;
  }
  .mcp-error {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--err, #d95f4e);
  }
  .mcp-empty {
    padding: 14px;
    margin: 0;
    font-size: 13px;
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .fld-wide {
    grid-column: 1 / -1;
  }
  .fld-label {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--fg-3);
  }
  .input {
    background: rgba(18, 19, 21, 0.7);
    border: 1px solid #26282c;
    border-radius: 5px;
    color: var(--fg-1);
    font-family: inherit;
    font-size: 13px;
    padding: 6px 9px;
    outline: none;
  }
  .input:focus {
    border-color: #3d4148;
  }
  .input.mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  textarea.input {
    resize: vertical;
    min-height: 34px;
  }
  .seg-sm .seg-item {
    padding: 3px 10px;
    font-size: 11px;
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--fg-3);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-1);
  }
  .icon-btn.danger:hover {
    color: var(--err, #d95f4e);
  }
</style>
