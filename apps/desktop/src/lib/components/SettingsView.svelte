<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import AddProviderSheet from './AddProviderSheet.svelte';
  import UsagePanel from './UsagePanel.svelte';
  import Popover from './Popover.svelte';
  import type {
    AntigravityStatus,
    AppInfo,
    ClaudeLoginInfo,
    McpServerConfig,
    McpServerInfo,
    NousLoginInfo,
    ProviderInfo,
    ProviderModel,
    ProviderPreset,
    SkillInfo,
  } from '../ipc';

  let providers = $state<ProviderInfo[]>([]);
  let presets = $state<ProviderPreset[]>([]);
  let skills = $state<SkillInfo[]>([]);
  let skillAuto = $state(true);
  let discordOn = $state(true);
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

  let agStatus = $state<AntigravityStatus | null>(null);
  let agBusy = $state<'install' | 'login' | 'refresh' | 'logout' | null>(null);
  let agError = $state<string | null>(null);
  let agProgress = $state<{ downloaded: number; total: number | null } | null>(null);
  let agLoginUrl = $state<string | null>(null);
  let agCopied = $state(false);
  let agUnlisten: UnlistenFn | null = null;

  let keyDrafts = $state<Record<string, string>>({});
  let keyBusy = $state<string | null>(null);

  import { THEMES } from '../themes';
  import { SETTINGS_SECTIONS } from '../settings';

  const MOTION_MODES = [
    { id: 'system', label: 'System', hint: 'follow the OS animation setting' },
    { id: 'full', label: 'Full', hint: 'always animate' },
    { id: 'reduced', label: 'Reduced', hint: 'settle all motion' },
  ] as const;

  const SECTIONS = SETTINGS_SECTIONS;

  const active = $derived(
    SECTIONS.some((s) => s.id === session.settingsSection) ? session.settingsSection : 'appearance',
  );
  let paneEl = $state<HTMLDivElement | null>(null);

  $effect(() => {
    active;
    paneEl?.scrollTo({ top: 0 });
  });

  async function refresh(): Promise<void> {
    if (!inTauri) return;
    const m = await import('../ipc');
    providers = await m.providersList().catch(() => []);
    presets = await m.providerPresets().catch(() => []);
    skills = await m.skillsList().catch(() => []);
    skillAuto = await m.skillsAutoGet().catch(() => true);
    discordOn = await m.presenceGet().catch(() => true);
    appInfo = await m.appInfo().catch(() => null);
    if (providers.some((p) => p.kind === 'acp')) {
      agStatus = await m.antigravityStatus().catch(() => null);
    }
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
      agUnlisten = await listen<{
        kind: string;
        downloaded?: number;
        total?: number | null;
        url?: string;
      }>('antigravity-event', (e) => {
        if (e.payload.kind === 'download') {
          agProgress = { downloaded: e.payload.downloaded ?? 0, total: e.payload.total ?? null };
        } else if (e.payload.kind === 'login_url' && e.payload.url) {
          agLoginUrl = e.payload.url;
        }
      });
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
    agUnlisten?.();
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

  let presetQuery = $state('');

  const PRESET_GROUPS = [
    { key: 'first-party', label: 'First-party' },
    { key: 'router', label: 'Routers' },
    { key: 'hosted', label: 'Hosted inference' },
    { key: 'local', label: 'Local' },
    { key: 'custom', label: 'Custom' },
  ] as const;

  // The custom row is a form, not a fixed provider: it stays available no
  // matter how many endpoints were already added under their own names.
  const addablePresets = $derived(
    presets.filter(
      (p) => p.group === 'custom' || !providers.some((q) => q.name === p.name),
    ),
  );

  const filteredPresets = $derived.by(() => {
    const q = presetQuery.trim().toLowerCase();
    if (!q) return addablePresets;
    return addablePresets.filter(
      (p) =>
        p.label.toLowerCase().includes(q) ||
        p.name.toLowerCase().includes(q) ||
        p.base_url.toLowerCase().includes(q) ||
        p.model.toLowerCase().includes(q),
    );
  });

  const presetGroupCount = $derived(
    PRESET_GROUPS.filter((g) => filteredPresets.some((p) => p.group === g.key)).length,
  );

  function presetsInGroup(key: string): ProviderPreset[] {
    return filteredPresets.filter((p) => p.group === key);
  }

  const presetLabels = $derived(new Map(presets.map((p) => [p.name, p.label])));

  function providerLabel(name: string): string {
    return presetLabels.get(name) ?? name;
  }

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

  let adding = $state<ProviderPreset | null>(null);
  let justAdded = $state<string | null>(null);
  let modelLists = $state<Record<string, ProviderModel[]>>({});
  let modelBusy = $state<string | null>(null);

  function openAdd(preset: ProviderPreset): void {
    // Local servers and the ACP agent need no key or model decision, so they
    // stay one click.
    if (preset.auth === 'none' || preset.auth === 'agent') {
      void addPreset(preset);
      return;
    }
    error = null;
    adding = preset;
  }

  async function addedProvider(name: string, model: string, useNow: boolean): Promise<void> {
    adding = null;
    justAdded = name;
    await refresh();
    if (useNow) await session.setModel(model, name).catch(() => {});
    setTimeout(() => (justAdded = null), 2600);
  }

  async function loadProviderModels(name: string): Promise<void> {
    modelBusy = name;
    try {
      const m = await import('../ipc');
      const list = await m.modelsList(name);
      modelLists = { ...modelLists, [name]: list };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      modelBusy = null;
    }
  }

  function onModelChange(provider: string, value: string): void {
    if (value === '__load') void loadProviderModels(provider);
    else if (value) void pickModel(provider, value);
  }

  async function pickModel(provider: string, model: string): Promise<void> {
    await session.setModel(model, provider).catch((e: unknown) => {
      error = e instanceof Error ? e.message : String(e);
    });
    await refresh();
  }

  /** Two-step remove: the first click asks, the second takes it off the account. */
  let confirmRemove = $state<string | null>(null);
  let confirmTimer: ReturnType<typeof setTimeout> | null = null;

  async function removeProvider(p: ProviderInfo): Promise<void> {
    if (confirmRemove !== p.name) {
      confirmRemove = p.name;
      if (confirmTimer) clearTimeout(confirmTimer);
      confirmTimer = setTimeout(() => (confirmRemove = null), 4000);
      return;
    }
    confirmRemove = null;
    if (confirmTimer) clearTimeout(confirmTimer);
    const m = await import('../ipc');
    error = null;
    try {
      // The Google agent keeps its sign-in outside the keychain, so sign out
      // first; provider_remove clears stored keys and OAuth tokens itself.
      if (p.kind === 'acp') await m.antigravityLogout().catch(() => {});
      await m.providerRemove(p.name);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
    await refresh();
    void session.refreshModels();
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

  async function setDiscordPresence(enabled: boolean): Promise<void> {
    discordOn = enabled;
    const m = await import('../ipc');
    await m.presenceSet(enabled).catch(() => {});
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

  async function agRun(
    kind: 'install' | 'login' | 'refresh' | 'logout',
    call: (m: typeof import('../ipc')) => Promise<AntigravityStatus>,
  ): Promise<void> {
    agBusy = kind;
    agError = null;
    try {
      const m = await import('../ipc');
      agStatus = await call(m);
      if (kind !== 'install') await refresh();
    } catch (e) {
      agError = e instanceof Error ? e.message : String(e);
    } finally {
      agBusy = null;
      agProgress = null;
      agLoginUrl = null;
      agCopied = false;
    }
  }

  async function agCancelLogin(): Promise<void> {
    const m = await import('../ipc');
    await m.antigravityLoginCancel().catch(() => {});
  }

  async function agCopyLink(): Promise<void> {
    if (!agLoginUrl) return;
    agCopied = await navigator.clipboard
      .writeText(agLoginUrl)
      .then(() => true)
      .catch(() => false);
  }

  function agProgressLabel(): string {
    if (!agProgress) return 'Downloading…';
    const mb = (n: number) => (n / 1048576).toFixed(0);
    return agProgress.total
      ? `Downloading… ${Math.round((agProgress.downloaded / agProgress.total) * 100)}%`
      : `Downloading… ${mb(agProgress.downloaded)} MB`;
  }

  function authLabel(p: ProviderInfo): string {
    if (p.kind === 'acp') return p.auth_kind === 'oauth' ? 'google sign-in' : 'not signed in';
    if (p.auth_kind === 'oauth') return 'subscription';
    if (p.auth_kind === 'api_key') return 'api key';
    if (p.auth_kind === 'env') return 'env key';
    return 'not connected';
  }

  /** What the endpoint speaks, in words rather than the internal kind. */
  function kindLabel(p: ProviderInfo): string {
    if (p.kind === 'acp') return 'Google Antigravity';
    if (p.kind === 'anthropic') return 'Anthropic-compatible';
    return 'OpenAI-compatible';
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

  function addContext7(): void {
    if (mcpRows.some((r) => r.name.toLowerCase() === 'context7')) {
      mcpError = 'context7 is already configured';
      return;
    }
    mcpRows = [
      ...mcpRows,
      {
        name: 'context7',
        kind: 'http',
        enabled: true,
        state: 'new',
        tools: 0,
        info: '',
        command: null,
        args: [],
        env: {},
        url: 'https://mcp.context7.com/mcp',
        headers: {},
      },
    ];
    mcpError = null;
    queueMcpSave();
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

<div class="settings glass-pane">
  <div class="pane" bind:this={paneEl}>
  {#if session.sidebarCollapsed}
    <nav class="dock" aria-label="Settings sections">
      {#each SECTIONS as sec (sec.id)}
        <button
          class="dock-item"
          class:active={active === sec.id}
          aria-current={active === sec.id ? 'page' : undefined}
          onclick={() => (session.settingsSection = sec.id)}
          title={sec.label}
        >
          <Icon name={sec.icon} size={14} />
          <span>{sec.label}</span>
        </button>
      {/each}
    </nav>
  {/if}
  <div class="settings-inner">

    <section class="group" id="appearance" hidden={active !== 'appearance'}>
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
        <div class="density-row">
          <span class="density-label">Density</span>
          <div class="range" role="group" aria-label="Transcript density">
            <button
              class="range-btn"
              class:active={session.density === 'comfortable'}
              onclick={() => session.setDensity('comfortable')}
              aria-pressed={session.density === 'comfortable'}
            >
              Cosy
            </button>
            <button
              class="range-btn"
              class:active={session.density === 'compact'}
              onclick={() => session.setDensity('compact')}
              aria-pressed={session.density === 'compact'}
            >
              Compact
            </button>
          </div>
          <span class="density-hint">Trims vertical rhythm in the transcript.</span>
        </div>
        <div class="density-row">
          <span class="density-label">Motion</span>
          <div class="range" role="group" aria-label="Motion">
            {#each MOTION_MODES as m (m.id)}
              <button
                class="range-btn"
                class:active={session.motion === m.id}
                onclick={() => session.setMotion(m.id)}
                aria-pressed={session.motion === m.id}
                title={m.hint}
              >
                {m.label}
              </button>
            {/each}
          </div>
          <span class="density-hint">
            {session.motion === 'system' && session.ambientMotion === 'off'
              ? 'Your system asks for less motion — fades and drift are settling, the cat still blinks.'
              : 'System follows your OS; Full keeps fades and drift on; Reduced settles everything.'}
          </span>
        </div>
        <label class="pref-row">
          <span class="pref-text">
            <span class="pref-name">
              Glass effect <span class="badge exp">experimental</span>
            </span>
            <span class="pref-hint">
              Floating frosted panes over a soft ambient light that follows your cursor.
            </span>
          </span>
          <input
            class="switch"
            type="checkbox"
            checked={session.glass}
            onchange={(e) => session.setGlass(e.currentTarget.checked)}
          />
        </label>
      </div>
    </section>

    <section class="group" id="providers" hidden={active !== 'providers'}>
      <h2 class="group-title">Providers</h2>
      <p class="group-hint">
        Connect a provider to start chatting. Keys stay in the OS keychain;
        Remove forgets a provider, its key and its sign-in.
      </p>

      {#if !inTauri}
        <p class="muted">Provider management needs the desktop app.</p>
      {:else}
        {#if providers.length === 0}
          <p class="muted">No providers yet — pick one below to get started.</p>
        {/if}
        <div class="provider-list">
          {#each providers as p (p.name)}
            {@const connected = p.auth_kind !== 'none'}
            {@const isAcp = p.kind === 'acp'}
            <article class="provider" class:live={connected} class:fresh={justAdded === p.name}>
              <header class="provider-head">
                <span class="status-dot" class:on={connected} aria-hidden="true"></span>
                <span class="provider-name">{providerLabel(p.name)}</span>
                {#if providerLabel(p.name) !== p.name}
                  <span class="provider-slug mono">{p.name}</span>
                {/if}
                {#if p.is_default}
                  <span class="badge">default</span>
                {/if}
                <span class="badge" class:connected>{authLabel(p)}</span>
                <span class="grow"></span>
                <button
                  class="remove-btn"
                  class:confirming={confirmRemove === p.name}
                  onclick={() => removeProvider(p)}
                  title="Remove this provider and forget its key or sign-in"
                >
                  {confirmRemove === p.name ? 'Confirm remove' : 'Remove'}
                </button>
              </header>

              <p class="provider-meta mono" title={p.base_url}>
                {kindLabel(p)}{#if p.base_url} · {p.base_url}{/if}
              </p>

              {#if isAcp}
                {#if agStatus && !agStatus.supported}
                  <p class="provider-hint">Google doesn't publish Antigravity for this platform.</p>
                {:else if !agStatus?.installed}
                  <div class="provider-actions">
                    <button
                      class="btn primary"
                      onclick={() => agRun('install', (m) => m.antigravityInstall())}
                      disabled={agBusy !== null}
                    >
                      {agBusy === 'install' ? agProgressLabel() : 'Install Antigravity'}
                    </button>
                    <span class="provider-hint">~100 MB from Google, checksum-verified</span>
                  </div>
                {:else if agBusy === 'login'}
                  <div class="login-box">
                    <p class="login-line">Finish signing in with Google in your browser…</p>
                    <div class="provider-actions">
                      {#if agLoginUrl}
                        <button class="btn" onclick={agCopyLink}>
                          {agCopied ? 'Copied' : 'Copy sign-in link'}
                        </button>
                      {/if}
                      <button class="btn ghost" onclick={agCancelLogin}>Cancel</button>
                    </div>
                  </div>
                {:else if !agStatus.signed_in}
                  <div class="provider-actions">
                    <button
                      class="btn primary"
                      onclick={() => agRun('login', (m) => m.antigravityLogin())}
                      disabled={agBusy !== null}
                    >
                      Sign in with Google
                    </button>
                    <span class="provider-hint">Free / Pro / Ultra plan</span>
                  </div>
                {:else}
                  <p class="provider-hint">
                    Signed in · {agStatus.models} model{agStatus.models === 1 ? '' : 's'} · v{agStatus.installed}
                  </p>
                {/if}
                {#if agError}
                  <p class="error-text">{agError}</p>
                {/if}
              {:else}
                <div class="provider-actions">
                  <select
                    class="input model-select"
                    aria-label="Model for {providerLabel(p.name)}"
                    value={p.model}
                    onchange={(e) => onModelChange(p.name, e.currentTarget.value)}
                  >
                    {#if !(modelLists[p.name] ?? []).some((x) => x.id === p.model)}
                      <option value={p.model}>{p.model || 'no model'}</option>
                    {/if}
                    {#if modelBusy === p.name}
                      <option disabled>loading models…</option>
                    {:else if !modelLists[p.name]?.length}
                      <option value="__load">Load models…</option>
                    {/if}
                    {#each modelLists[p.name] ?? [] as m (m.id)}
                      <option value={m.id}>{m.id}</option>
                    {/each}
                  </select>

                  {#if p.name === 'nous' && !login}
                    <button
                      class="btn primary"
                      onclick={connectNous}
                      disabled={loginBusy || (p.auth_kind !== 'none' && p.auth_kind !== 'env')}
                    >
                      {p.auth_kind === 'none' ? 'Connect Nous Portal' : 'Reconnect'}
                    </button>
                  {/if}

                  {#if p.name === 'claude' && !claudeLogin}
                    <button class="btn primary" onclick={connectClaude} disabled={claudeBusy}>
                      {p.auth_kind === 'oauth' ? 'Reconnect' : 'Sign in with Claude'}
                    </button>
                  {/if}

                  {#if (p.auth_kind === 'none' || p.auth_kind === 'env' || p.auth_kind === 'api_key') && !(p.name === 'nous' && login) && !(p.name === 'claude' && claudeLogin)}
                    <input
                      class="input"
                      type="password"
                      placeholder={p.auth_kind === 'api_key' ? 'replace key…' : 'API key…'}
                      bind:value={keyDrafts[p.name]}
                      onkeydown={(e) => e.key === 'Enter' && saveKey(p.name)}
                    />
                    <button
                      class="btn"
                      onclick={() => saveKey(p.name)}
                      disabled={!keyDrafts[p.name]?.trim() || keyBusy === p.name}
                    >
                      {keyBusy === p.name ? 'Saving…' : 'Save key'}
                    </button>
                  {/if}
                </div>

                {#if p.name === 'nous' && login}
                  <div class="login-box">
                    <p class="login-line">
                      Approve at
                      <a href={login.verification_url} target="_blank" rel="noreferrer">
                        {login.verification_url}
                      </a>
                      — code <code>{login.user_code}</code>
                    </p>
                    <div class="provider-actions">
                      <button class="btn ghost" onclick={cancelLogin}>Cancel</button>
                    </div>
                  </div>
                {/if}
                {#if p.name === 'nous' && loginError}
                  <p class="error-text">{loginError}</p>
                {/if}

                {#if p.name === 'claude' && claudeLogin}
                  <div class="login-box">
                    <p class="login-line">
                      Approve at
                      <a href={claudeLogin.authorize_url} target="_blank" rel="noreferrer">
                        {claudeLogin.authorize_url}
                      </a>
                    </p>
                    <div class="provider-actions">
                      <input
                        class="input"
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
                {/if}
                {#if p.name === 'claude' && claudeError}
                  <p class="error-text">{claudeError}</p>
                {/if}
              {/if}
            </article>
          {/each}

          {#if addablePresets.length > 0}
            <div class="preset-head">
              <p class="preset-title">
                Add a provider
                <span class="preset-count mono">{addablePresets.length} available</span>
              </p>
              {#if addablePresets.length > 6}
                <input
                  class="input preset-filter"
                  type="search"
                  placeholder="Filter providers…"
                  aria-label="Filter providers"
                  bind:value={presetQuery}
                />
              {/if}
            </div>
            {#each PRESET_GROUPS as group (group.key)}
              {@const items = presetsInGroup(group.key)}
              {#if items.length > 0}
                {#if presetGroupCount > 1}
                  <p class="preset-sub mono">{group.label}</p>
                {/if}
                <div class="preset-grid">
                  {#each items as preset (preset.name)}
                    <button
                      class="preset-card"
                      onclick={() => openAdd(preset)}
                      disabled={presetBusy === preset.name}
                      title={`${preset.base_url}
${preset.key_url ?? ''}`}
                    >
                      <span class="preset-name">{preset.label}</span>
                      <span class="preset-meta">
                        {preset.auth === 'none'
                          ? 'local · no key'
                          : preset.auth === 'agent'
                            ? 'Google sign-in'
                            : preset.auth === 'oauth'
                              ? 'subscription or key'
                              : (preset.env_hint ?? 'api key')}
                      </span>
                    </button>
                  {/each}
                </div>
              {/if}
            {/each}
            {#if filteredPresets.length === 0}
              <p class="muted">No provider matches “{presetQuery}”.</p>
            {/if}
          {/if}
        </div>
      {/if}
      {#if error}
        <p class="error-text">{error}</p>
      {/if}
    </section>

    {#if adding}
      <AddProviderSheet
        preset={adding}
        existingNames={providers.map((p) => p.name)}
        onClose={() => (adding = null)}
        onAdded={addedProvider}
      />
    {/if}

    <section class="group" id="skills" hidden={active !== 'skills'}>
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
          <div class="empty-block">
            <p class="empty-title">No skills yet</p>
            <p class="empty-copy">
              A skill is a short Markdown note the agent writes after solving something
              non-obvious — the command that finally worked, the version quirk, the
              pitfall. Matching skills ride along in the prompt next time.
            </p>
            <p class="empty-example mono">
              example · nextjs-route-handlers — triggers: app router, route handler
            </p>
            {#if !skillAuto}
              <p class="empty-copy">
                Auto-create is off, so nothing will be learned from your sessions.
                <button class="btn sm" onclick={() => setAutoSkills(true)}>Turn it on</button>
              </p>
            {:else}
              <p class="empty-copy dim">
                Skills appear once a session reaches three turns, or when you ask the agent
                to remember something.
              </p>
            {/if}
          </div>
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

    <section class="group" id="spending" hidden={active !== 'spending'}>
      <h2 class="group-title">Spending</h2>
      <p class="group-hint">
        Optional ceilings, checked before a turn starts. Nothing is ever blocked
        silently — hitting the limit asks first.
      </p>
      <div class="card spend-card">
        <div class="spend-row">
          <label class="spend-field">
            <span class="spend-label">Per session</span>
            <div class="money-input">
              <span class="money-sym mono">$</span>
              <input
                class="input"
                type="number"
                min="0"
                step="0.5"
                placeholder="no limit"
                value={session.prefs.budget_session_usd ?? ''}
                onchange={(e) => {
                  const v = Number(e.currentTarget.value);
                  void session.savePrefs({
                    budget_session_usd:
                      e.currentTarget.value.trim() === '' || !Number.isFinite(v) || v <= 0
                        ? null
                        : v,
                  });
                }}
              />
            </div>
          </label>
          <label class="spend-field">
            <span class="spend-label">Per day</span>
            <div class="money-input">
              <span class="money-sym mono">$</span>
              <input
                class="input"
                type="number"
                min="0"
                step="1"
                placeholder="no limit"
                value={session.prefs.budget_day_usd ?? ''}
                onchange={(e) => {
                  const v = Number(e.currentTarget.value);
                  void session.savePrefs({
                    budget_day_usd:
                      e.currentTarget.value.trim() === '' || !Number.isFinite(v) || v <= 0
                        ? null
                        : v,
                  });
                }}
              />
            </div>
          </label>
        </div>
        <p class="spend-now mono">
          this session {session.costUsd > 0 ? `$${session.costUsd.toFixed(4)}` : '$0'}
          {#if session.todayCostUsd > 0}
            · today ${session.todayCostUsd.toFixed(2)}
          {/if}
        </p>
        <div class="toggle-row">
          <label class="toggle">
            <input
              type="checkbox"
              checked={session.prefs.sound_on_done}
              onchange={(e) => void session.savePrefs({ sound_on_done: e.currentTarget.checked })}
            />
            <span>Chime when a turn finishes in the background</span>
          </label>
          <label class="toggle">
            <input
              type="checkbox"
              checked={session.prefs.notify_on_done}
              onchange={(e) => void session.savePrefs({ notify_on_done: e.currentTarget.checked })}
            />
            <span>Flash the taskbar and show a toast</span>
          </label>
        </div>
      </div>
    </section>

    <section class="group" id="usage" hidden={active !== 'usage'}>
      <h2 class="group-title">Usage</h2>
      <p class="group-hint">
        Read from the local request ledger — tokens, cache hits, latency and price per
        call. Never uploaded.
      </p>
      <div class="card">
        <UsagePanel />
      </div>
    </section>

    <section class="group" id="mcp" hidden={active !== 'mcp'}>
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
          <div class="empty-block mcp-empty">
            <p class="empty-title">No MCP servers yet</p>
            <p class="empty-body">
              A server exposes extra tools to every session — docs search, a database,
              your issue tracker. Add one below, or start with Context7 for library docs.
            </p>
            <div class="empty-actions">
              <button class="btn sm" onclick={() => (showMcpAdd = true)}>
                <Icon name="plus" size={11} />
                Add a server
              </button>
              <button class="btn ghost sm" onclick={addContext7}>
                Add Context7 docs
              </button>
            </div>
          </div>
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
        <div class="mcp-add-row">
          <button class="btn ghost mcp-add-btn" onclick={() => (showMcpAdd = true)}>
            <Icon name="plus" size={13} /> Add MCP server
          </button>
          <button
            class="btn ghost mcp-add-btn"
            onclick={addContext7}
            title="Add the Context7 docs server"
          >
            <Icon name="book" size={13} /> Add Context7 docs
          </button>
          <span class="spacer"></span>
          <Popover align="right" width="420px">
            {#snippet trigger()}
              <Icon name="info" size={12} /> docs info
            {/snippet}
            {#snippet panel({ close })}
              <div class="ctx7">
                <div class="ctx7-head">
                  <Icon name="book" size={13} />
                  <span class="ctx7-title">Context7 docs</span>
                  <span class="mcp-spacer"></span>
                  <button class="icon-btn" onclick={close} aria-label="Close">
                    <Icon name="x" size={11} />
                  </button>
                </div>
                <p class="ctx7-lede">
                  Current, version-specific documentation for ~163k libraries, fetched while
                  the agent works instead of guessed from its training cut-off.
                </p>
                <div class="ctx7-tool">
                  <code class="mono">resolve-library-id</code>
                  <span>
                    turns a name into a Context7 id (<span class="mono">Next.js</span> →
                    <span class="mono">/vercel/next.js</span>). Runs first, every time.
                  </span>
                </div>
                <div class="ctx7-tool">
                  <code class="mono">query-docs</code>
                  <span>
                    returns that library's current docs and real code examples for one
                    question.
                  </span>
                </div>
                <p class="ctx7-ask">
                  Both are read-only, so they never stop to ask permission — which also means
                  the agent decides on its own whether to use them. Say it explicitly:
                  <em>“check the current Next.js docs for route handlers”</em> or
                  <em>“use context7 to look up the Prisma upsert API”</em>.
                </p>
              </div>
            {/snippet}
          </Popover>
        </div>
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

    <section class="group" id="about" hidden={active !== 'about'}>
      <h2 class="group-title">About</h2>
      <div class="card">
        {#if appInfo}
          <dl class="about-grid">
            <dt>Version</dt><dd class="mono">{appInfo.version}</dd>
            <dt>Platform</dt><dd class="mono">{appInfo.platform} · {appInfo.arch}</dd>
            <dt>Data</dt><dd class="mono about-path" title={appInfo.data_dir}>{appInfo.data_dir}</dd>
          </dl>
          <label class="pref-row">
            <span class="pref-text">
              <span class="pref-name">Discord Rich Presence</span>
              <span class="pref-hint">show hate-agent as your active activity, with time elapsed</span>
            </span>
            <input
              class="switch"
              type="checkbox"
              checked={discordOn}
              onchange={(e) => setDiscordPresence(e.currentTarget.checked)}
            />
          </label>
        {:else}
          <p class="muted">App info unavailable outside the desktop app.</p>
        {/if}
      </div>
    </section>
  </div>
  </div>
</div>

<style>
  .settings {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    background: var(--bg-0);
  }
  .dock {
    position: sticky;
    top: 12px;
    z-index: 10;
    display: flex;
    width: fit-content;
    max-width: calc(100% - 32px);
    margin: 12px auto 0;
    gap: 2px;
    padding: 4px;
    overflow-x: auto;
    scrollbar-width: none;
    background: color-mix(in srgb, var(--bg-1) 86%, transparent);
    backdrop-filter: blur(12px);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-pill);
    box-shadow: var(--el-2);
  }
  .dock::-webkit-scrollbar {
    display: none;
  }
  .dock-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: none;
    height: 30px;
    padding: 0 12px;
    border-radius: var(--radius-pill);
    font-size: 12.5px;
    color: var(--fg-2);
    transition:
      background var(--motion),
      color var(--motion);
  }
  .dock-item:hover {
    color: var(--fg-0);
  }
  .dock-item.active {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .pane {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
  }
  .settings-inner {
    max-width: 720px;
    margin: 0 auto;
    padding: 28px 36px 88px;
  }
  .group[hidden] {
    display: none;
  }
  .group-title {
    font-family: var(--font-ui);
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--fg-0);
    margin: 0 0 6px;
  }
  .group-hint {
    max-width: 64ch;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--fg-2);
    margin: 0 0 14px;
    line-height: 1.6;
  }

  .card,
  .provider {
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--fg-0) 2.5%, var(--bg-1)),
      var(--bg-1)
    );
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--el-1);
  }
  .density-row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .density-label {
    font-size: 11.5px;
    color: var(--fg-1);
  }
  .density-hint {
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .range {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .range-btn {
    padding: 3px 9px;
    border-radius: 6px;
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .range-btn.active {
    background: var(--bg-0);
    color: var(--fg-0);
  }
  .spend-card {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .spend-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 12px;
  }
  .spend-field {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .spend-label {
    font-size: 11px;
    color: var(--fg-2);
  }
  .money-input {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .money-input .input {
    flex: 1 1 auto;
    min-width: 0;
  }
  .money-sym {
    font-size: 12px;
    color: var(--fg-3);
    flex: none;
  }
  .spend-now {
    margin: 0;
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .toggle-row {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11.5px;
    color: var(--fg-2);
    cursor: pointer;
  }
  .toggle input {
    accent-color: var(--accent);
    flex: none;
  }
  .card {
    padding: 16px 18px;
  }
  /* These cards supply their own row padding, so the shell stays flush. */
  .card:has(> .skill-toggle) {
    padding: 0;
  }

  .provider-name,
  .provider-meta,
  .about-grid dd,
  .login-line,
  .login-line code {
    font-family: var(--font-mono);
  }
  .seg {
    display: inline-flex;
    gap: 3px;
    padding: 3px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .seg-item {
    font-family: var(--font-mono);
    font-size: 12px;
    padding: 6px 14px;
    border-radius: 7px;
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
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 32%, transparent);
    color: var(--accent);
  }
  .theme-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(90px, 1fr));
    gap: 10px;
  }
  .theme-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
    text-align: left;
    transition:
      border-color var(--motion),
      transform var(--motion),
      box-shadow var(--motion);
  }
  .theme-card:hover {
    border-color: var(--border-strong);
    transform: translateY(-1px);
  }
  .theme-card.active {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .theme-prev {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: 5px;
    height: 46px;
    padding: 9px;
    border-radius: 8px;
    border: 1px solid rgba(128, 128, 128, 0.2);
    overflow: hidden;
  }
  .theme-bar {
    height: 5px;
    width: 55%;
    border-radius: 3px;
  }
  .theme-line {
    height: 3px;
    width: 80%;
    border-radius: 3px;
    opacity: 0.5;
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
    position: relative;
    overflow: hidden;
    padding: 14px 16px 14px 18px;
    background: linear-gradient(180deg, var(--bg-2), var(--bg-1));
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--el-1);
    transition:
      border-color var(--motion),
      box-shadow var(--motion);
  }
  .provider:hover {
    border-color: var(--border-strong);
  }
  .provider::before {
    content: '';
    position: absolute;
    inset: 12px auto 12px 0;
    width: 2px;
    border-radius: 2px;
    background: var(--border-strong);
  }
  .provider.live::before {
    background: color-mix(in srgb, var(--ok) 70%, transparent);
  }
  .provider.fresh {
    border-color: var(--accent-dim);
    box-shadow: 0 0 0 1px var(--accent-dim);
  }
  .provider-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 7px;
    margin-bottom: 4px;
  }
  .status-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
    background: var(--fg-4, var(--fg-3));
    opacity: 0.6;
  }
  .status-dot.on {
    opacity: 1;
    background: var(--ok, #4ade80);
    box-shadow: 0 0 6px color-mix(in srgb, var(--ok, #4ade80) 55%, transparent);
  }
  .provider-name {
    font-size: 13px;
    font-weight: 500;
    color: var(--fg-0);
  }
  .badge {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.05em;
    padding: 2px 8px;
    border-radius: var(--radius-pill);
    background: var(--bg-3);
    color: var(--fg-2);
    text-transform: uppercase;
  }
  .badge.exp {
    margin-left: 6px;
    padding: 1px 6px;
    font-size: 9px;
    vertical-align: 1px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .badge.connected {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .grow {
    flex: 1 1 auto;
  }
  .remove-btn {
    flex: none;
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 7px;
    border: 1px solid transparent;
    color: var(--fg-3);
    transition:
      color var(--motion),
      background var(--motion),
      border-color var(--motion);
  }
  .remove-btn:hover {
    color: var(--err);
    border-color: color-mix(in srgb, var(--err) 40%, transparent);
    background: color-mix(in srgb, var(--err) 10%, transparent);
  }
  .remove-btn.confirming {
    color: #fff;
    background: var(--err);
    border-color: var(--err);
  }
  .provider-meta {
    font-size: 11px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0;
  }
  .provider-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 12px;
  }
  .provider-actions .input {
    flex: 1 1 200px;
    min-width: 160px;
    width: auto;
  }
  .model-select {
    flex: 0 1 260px;
    min-width: 170px;
    padding: 4px 8px;
    font-size: 11.5px;
  }
  .login-box {
    margin-top: 10px;
    padding: 4px 0 0;
  }
  .login-box .provider-actions {
    margin-top: 8px;
  }
  .login-line {
    font-size: 12px;
    color: var(--fg-1);
    margin: 0 0 8px;
    line-height: 1.65;
    overflow-wrap: anywhere;
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
    font-size: 11px;
    color: var(--fg-3);
    align-self: center;
  }
  .preset-head {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 20px 0 4px;
    flex-wrap: wrap;
  }
  .preset-title {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--fg-3);
    margin: 0;
  }
  .preset-count {
    letter-spacing: 0;
    text-transform: none;
    color: var(--fg-4, var(--fg-3));
    opacity: 0.7;
  }
  .preset-filter {
    margin-left: auto;
    width: 200px;
    padding: 4px 10px;
    font-size: 11.5px;
  }
  .preset-sub {
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-4, var(--fg-3));
    opacity: 0.75;
    margin: 14px 0 6px;
  }
  .provider-slug {
    font-size: 10px;
    color: var(--fg-4, var(--fg-3));
    opacity: 0.65;
  }
  .preset-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(128px, 1fr));
    gap: 8px;
  }
  .preset-card {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 10px 12px;
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--fg-0) 2.5%, var(--bg-1)),
      var(--bg-1)
    );
    border: 1px solid var(--border);
    border-radius: 12px;
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
    opacity: 0.45;
    cursor: default;
  }
  .preset-name {
    font-size: 12px;
    color: var(--fg-0);
    overflow-wrap: anywhere;
  }
  .preset-meta {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--fg-3);
    overflow-wrap: anywhere;
  }
  .skill-toggle {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 12px 16px;
    font-size: 12px;
    color: var(--fg-1);
    cursor: pointer;
  }
  .skill-toggle input {
    accent-color: var(--accent);
    flex: none;
  }
  .empty-block {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 4px 16px 16px;
  }
  .card:has(> .skill-toggle) .empty-block {
    padding: 14px 16px 16px;
    border-top: 1px solid var(--border);
  }
  .empty-title {
    margin: 0;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--fg-0);
  }
  .empty-copy,
  .empty-body {
    margin: 0;
    max-width: 62ch;
    font-size: 11.5px;
    line-height: 1.65;
    color: var(--fg-2);
  }
  .empty-copy.dim {
    color: var(--fg-3);
  }
  .empty-example {
    margin: 0;
    padding: 7px 10px;
    font-size: 10.5px;
    color: var(--fg-2);
    background: var(--bg-0);
    border: 1px dashed var(--border);
    border-radius: var(--radius);
  }
  .empty-copy .btn {
    margin-left: 6px;
    vertical-align: middle;
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
    padding: 6px 12px 6px 16px;
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
    min-width: 0;
    text-align: left;
    background: none;
    border: none;
    padding: 4px 0;
    cursor: pointer;
  }
  .skill-title {
    font-size: 12px;
    color: var(--fg-0);
    overflow-wrap: anywhere;
  }
  .skill-meta {
    font-size: 10px;
    color: var(--fg-3);
    overflow-wrap: anywhere;
  }
  .skill-body {
    margin: 0;
    padding: 12px 16px;
    font-size: 11px;
    line-height: 1.6;
    color: var(--fg-1);
    background: var(--bg-0);
    border-bottom: 1px solid var(--border);
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
  }
  .spacer {
    flex: 1;
  }

  .error-text {
    margin: 10px 0 0;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--err);
  }
  .muted {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--fg-3);
  }
  .pref-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin: 16px -18px -16px;
    padding: 13px 18px;
    border-top: 1px solid var(--border);
    border-radius: 0 0 var(--radius-lg) var(--radius-lg);
    cursor: pointer;
    transition: background var(--motion);
  }
  .pref-row:hover {
    background: var(--bg-2);
  }
  .pref-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .pref-name {
    font-size: 13px;
    color: var(--fg-0);
  }
  .pref-hint {
    font-size: 11px;
    color: var(--fg-2);
  }
  .about-grid {
    display: grid;
    grid-template-columns: 84px minmax(0, 1fr);
    gap: 9px 14px;
    margin: 0;
    font-size: 12.5px;
  }
  .about-grid dt {
    font-family: var(--font-mono);
    color: var(--fg-3);
  }
  .about-grid dd {
    margin: 0;
    font-size: 12.5px;
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
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  .mcp-row:last-child {
    border-bottom: none;
  }
  .mcp-row.mcp-off {
    opacity: 0.55;
  }
  .mcp-refresh {
    font-size: 10.5px;
    padding: 2px 9px;
    margin-left: 2px;
    vertical-align: middle;
  }
  .mcp-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  .mcp-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--fg-3);
    flex: none;
  }
  .mcp-dot.dot-ok {
    background: color-mix(in srgb, var(--ok) 80%, transparent);
  }
  .mcp-dot.dot-busy {
    background: var(--warn);
    animation: mcp-pulse 1s ease-in-out infinite;
  }
  .mcp-dot.dot-err {
    background: var(--err);
  }
  @keyframes mcp-pulse {
    50% {
      opacity: 0.35;
    }
  }
  .mcp-name {
    font-size: 12.5px;
    color: var(--fg-0);
  }
  .mcp-kind {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--fg-3);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 6px;
  }
  .mcp-state {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .mcp-state.state-ok {
    color: color-mix(in srgb, var(--ok) 85%, var(--fg-2));
  }
  .mcp-state.state-err {
    color: var(--err);
  }
  .mcp-spacer {
    flex: 1;
  }
  .mcp-enable {
    display: flex;
    align-items: center;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-3);
    cursor: pointer;
    user-select: none;
  }
  .mcp-enable input {
    accent-color: var(--accent);
    flex: none;
  }
  .mcp-sub {
    margin: 5px 0 0 15px;
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mcp-info {
    color: var(--fg-3);
  }
  .mcp-editor {
    margin: 12px 0 2px 15px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .mcp-add {
    margin-top: 10px;
    padding: 0;
  }
  .mcp-add-grid {
    display: grid;
    grid-template-columns: 1fr 132px;
    gap: 12px 14px;
    padding: 16px 18px 12px;
  }
  .mcp-add-actions {
    display: flex;
    gap: 8px;
    padding: 0 18px 16px;
  }
  .mcp-add-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .mcp-add-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
  }
  .ctx7 {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px 14px;
  }
  .ctx7-head {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--accent);
  }
  .ctx7-title {
    font-size: 12px;
    font-weight: 650;
    letter-spacing: 0.02em;
    color: var(--fg-0);
  }
  .ctx7-lede {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.55;
    color: var(--fg-2);
  }
  .ctx7-tool {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding-left: 9px;
    border-left: 2px solid var(--border);
  }
  .ctx7-tool code {
    font-size: 11px;
    color: var(--accent);
  }
  .ctx7-tool span {
    font-size: 11px;
    line-height: 1.5;
    color: var(--fg-2);
  }
  .ctx7-tool span :global(.mono) {
    color: var(--fg-1);
  }
  .ctx7-ask {
    margin: 0;
    padding-top: 8px;
    border-top: 1px solid var(--border);
    font-size: 11px;
    line-height: 1.6;
    color: var(--fg-2);
  }
  .ctx7-ask em {
    color: var(--fg-0);
    font-style: normal;
    white-space: nowrap;
  }
  .mcp-foot {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 12px;
  }
  .mcp-error {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--err);
  }
  .empty-actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    flex-wrap: wrap;
  }
  .mcp-empty {
    padding: 0;
    margin: 0;
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: 5px;
    min-width: 0;
  }
  .fld-wide {
    grid-column: 1 / -1;
  }
  .fld-label {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .input.mono {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  textarea.input {
    resize: vertical;
    min-height: 36px;
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
    border-radius: 6px;
    background: transparent;
    color: var(--fg-3);
    cursor: pointer;
    transition:
      background var(--motion),
      color var(--motion);
  }
  .icon-btn:hover {
    background: var(--bg-2);
    color: var(--fg-1);
  }
  .icon-btn.danger:hover {
    color: var(--err);
  }
  @media (max-width: 820px) {
    .dock-item span {
      display: none;
    }
  }
  @media (max-width: 640px) {
    .settings-inner {
      padding: 24px 16px 64px;
    }
    .preset-filter {
      width: 100%;
      margin-left: 0;
    }
    .mcp-add-grid {
      grid-template-columns: 1fr;
    }
    .about-grid {
      grid-template-columns: 74px minmax(0, 1fr);
    }
    .theme-grid {
      grid-template-columns: repeat(auto-fit, minmax(96px, 1fr));
    }
  }
</style>
