<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import ProvidersPanel from './ProvidersPanel.svelte';
  import UsagePanel from './UsagePanel.svelte';
  import Popover from './Popover.svelte';
  import type {
    AppInfo,
    McpServerConfig,
    McpServerInfo,
    SkillInfo,
  } from '../ipc';

  let skills = $state<SkillInfo[]>([]);
  let skillAuto = $state(true);
  let discordOn = $state(true);
  let skillOpen = $state<string | null>(null);
  let skillBodies = $state<Record<string, string>>({});
  let appInfo = $state<AppInfo | null>(null);
  let inTauri = $state(false);
  let error = $state<string | null>(null);

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
    skills = await m.skillsList().catch(() => []);
    skillAuto = await m.skillsAutoGet().catch(() => true);
    discordOn = await m.presenceGet().catch(() => true);
    appInfo = await m.appInfo().catch(() => null);
    await loadMcp();
  }

  onMount(async () => {
    try {
      const m = await import('../ipc');
      await m.appInfo();
      inTauri = true;
      await refresh();
    } catch {
    }
  });

  onDestroy(() => {
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

  function addWebSearch(): void {
    if (mcpRows.some((r) => r.name.toLowerCase() === 'web_search' || r.args?.[0] === '--mcp-web-search')) {
      mcpError = 'web_search is already configured';
      return;
    }
    mcpRows = [
      ...mcpRows,
      {
        name: 'web_search',
        kind: 'stdio',
        enabled: true,
        state: 'new',
        tools: 0,
        info: '',
        command: 'builtin:web-search',
        args: ['--mcp-web-search'],
        env: {},
        url: null,
        headers: {},
      },
    ];
    mcpError = null;
    queueMcpSave();
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

  // Built-in web search: any key below upgrades the backend from scraping
  // rate-limited engines to a real search API.
  const WEB_SEARCH_KEYS = [
    { key: 'BRAVE_API_KEY', label: 'Brave Search key', hint: 'api-dashboard.search.brave.com' },
    { key: 'TAVILY_API_KEY', label: 'Tavily key', hint: 'tavily.com' },
    { key: 'EXA_API_KEY', label: 'Exa key', hint: 'exa.ai' },
    { key: 'SERPER_API_KEY', label: 'Serper key', hint: 'serper.dev' },
    { key: 'SEARXNG_URL', label: 'SearXNG URL', hint: 'http://localhost:8080' }
  ] as const;

  function webSearchRow(): McpServerInfo | undefined {
    return mcpRows.find((r) => r.kind === 'stdio' && r.args?.[0] === '--mcp-web-search');
  }

  function setWebSearchEnv(key: string, value: string): void {
    const row = webSearchRow();
    if (!row) return;
    const env: Record<string, string> = { ...(row.env ?? {}) };
    const trimmed = value.trim();
    if (trimmed) env[key] = trimmed;
    else delete env[key];
    row.env = env;
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
  <div class="settings-inner" class:wide={active === 'providers'}>

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
      <ProvidersPanel active={active === 'providers'} />
    </section>

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
        Their tools reach the agent as        <code class="mono">mcp__server__tool</code>;
        anything not marked read-only asks permission, same as bash.
      </p>

      {#if webSearchRow()}
        <div class="card mcp-list web-search-keys">
          <div class="mcp-row">
            <div class="mcp-head">
              <span class="mcp-name mono">Built-in web search</span>
              <span class="mcp-kind">optional keys</span>
            </div>
            <p class="group-hint muted">
              With no key the agent scrapes DuckDuckGo, Bing, Mojeek, Hacker News and Wikipedia,
              which the engines throttle and bot-wall. Any key below makes that query go to a real
              search API first. Stored in the <code class="mono">web_search</code> server env.
            </p>
            {#each WEB_SEARCH_KEYS as field (field.key)}
              <label class="fld">
                <span class="fld-label">{field.label}</span>
                <input
                  class="input mono"
                  type={field.key === 'SEARXNG_URL' ? 'text' : 'password'}
                  autocomplete="off"
                  spellcheck="false"
                  placeholder={field.hint}
                  value={webSearchRow()?.env?.[field.key] ?? ''}
                  oninput={(e) =>
                    setWebSearchEnv(field.key, (e.currentTarget as HTMLInputElement).value)}
                />
              </label>
            {/each}
          </div>
        </div>
      {/if}

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
                {#if row.kind === 'stdio' && row.args?.[0] === '--mcp-web-search'}
                  built-in · free web search &amp; page fetch (no API key)
                {:else if row.kind === 'stdio'}
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
              <button class="btn ghost sm" onclick={addWebSearch}>
                Add Web Search
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
            onclick={addWebSearch}
            title="Add the free built-in web search server"
          >
            <Icon name="search" size={13} /> Add Web Search
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
    transition: max-width var(--motion);
  }
  .settings-inner.wide {
    max-width: 1020px;
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

  .card {
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

  .about-grid dd {
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
