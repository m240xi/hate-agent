<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import ProviderLogo from './ProviderLogo.svelte';
  import type {
    AntigravityStatus,
    ClaudeLoginInfo,
    HardwareSpecs,
    LocalModelStatus,
    LocalRuntimeStatus,
    NousLoginInfo,
    ProviderInfo,
    ProviderModel,
    ProviderPreset,
  } from '../ipc';

  interface Props {
    active?: boolean;
  }

  let { active = true }: Props = $props();

  type FilterCat = 'all' | 'active' | 'first-party' | 'router' | 'hosted' | 'local' | 'custom';

  interface AtlasItem {
    key: string;
    label: string;
    group: 'first-party' | 'router' | 'hosted' | 'local' | 'custom';
    preset: ProviderPreset | null;
    installed: ProviderInfo | null;
    connected: boolean;
    isDefault: boolean;
    kind: string;
    baseUrl: string;
    envHint: string | null;
    keyUrl: string | null;
  }

  const GROUPS = [
    { key: 'first-party', label: 'Direct APIs', short: 'Direct' },
    { key: 'router', label: 'Routers', short: 'Routers' },
    { key: 'hosted', label: 'Hosted Inference', short: 'Hosted' },
    { key: 'local', label: 'Local Runtimes', short: 'Local' },
    { key: 'custom', label: 'Custom Endpoints', short: 'Custom' },
  ] as const;

  let providers = $state<ProviderInfo[]>([]);
  let presets = $state<ProviderPreset[]>([]);
  let inTauri = $state(false);
  let checkedEnv = $state(false);
  let error = $state<string | null>(null);

  let selectedKey = $state<string | null>(null);
  let query = $state('');
  let cat = $state<FilterCat>('all');
  let searchEl = $state<HTMLInputElement | null>(null);

  let login = $state<NousLoginInfo | null>(null);
  let loginBusy = $state(false);
  let loginError = $state<string | null>(null);
  let unlisten: UnlistenFn | null = null;

  let claudeLogin = $state<ClaudeLoginInfo | null>(null);
  let claudeCode = $state('');
  let claudeBusy = $state(false);
  let claudeError = $state<string | null>(null);

  let agStatus = $state<AntigravityStatus | null>(null);
  let agBusy = $state<'install' | 'login' | 'refresh' | 'logout' | null>(null);
  let agError = $state<string | null>(null);
  let agProgress = $state<{ downloaded: number; total: number | null } | null>(null);
  let agLoginUrl = $state<string | null>(null);
  let agCopied = $state(false);
  let agUnlisten: UnlistenFn | null = null;

  let localStatus = $state<LocalRuntimeStatus | null>(null);
  let localLoading = $state(false);
  let localBusy = $state<string | null>(null);
  let localError = $state<string | null>(null);
  let localNotice = $state<string | null>(null);
  let localDlProgress = $state<{
    modelId: string;
    downloadedMb: number;
    totalMb: number | null;
    pct: number;
  } | null>(null);
  let localRuntimeDl = $state<{
    backend: string;
    downloadedMb: number;
    totalMb: number | null;
  } | null>(null);
  let confirmDeleteModel = $state<string | null>(null);
  let confirmDeleteTimer: ReturnType<typeof setTimeout> | null = null;
  let localModelsCopied = $state(false);
  let localUnlisten: UnlistenFn | null = null;

  let keyDrafts = $state<Record<string, string>>({});
  let showInstalledKey = $state<Record<string, boolean>>({});
  let keyBusy = $state<string | null>(null);
  let logoutBusy = $state<string | null>(null);
  let activateBusy = $state<string | null>(null);

  let modelLists = $state<Record<string, ProviderModel[]>>({});
  let modelBusy = $state<string | null>(null);
  let modelFilter = $state('');

  let confirmRemove = $state<string | null>(null);
  let confirmTimer: ReturnType<typeof setTimeout> | null = null;
  let justAdded = $state<string | null>(null);
  let copiedUrl = $state(false);

  // Inline setup / custom endpoint state for the Stage
  let setupKey = $state('');
  let setupShowKey = $state(false);
  let setupModel = $state('');
  let setupBaseUrl = $state('');
  let setupKind = $state<'openai_compat' | 'anthropic'>('openai_compat');
  let setupName = $state('');
  let setupNameTouched = $state(false);
  let setupUseNow = $state(true);
  let setupShowAdvanced = $state(false);
  let setupModels = $state<ProviderModel[]>([]);
  let setupScanning = $state(false);
  let setupScanError = $state<string | null>(null);
  let setupBusy = $state(false);
  let setupError = $state<string | null>(null);

  const KINDS = [
    { id: 'openai_compat', label: 'OpenAI-compatible' },
    { id: 'anthropic', label: 'Anthropic-compatible' },
  ] as const;

  const customPreset = $derived(presets.find((p) => p.group === 'custom') ?? null);

  const atlasItems = $derived.by<AtlasItem[]>(() => {
    const byName = new Map(providers.map((p) => [p.name, p]));
    const seen = new Set<string>();
    const out: AtlasItem[] = [];

    for (const p of presets) {
      if (p.group === 'custom') continue;
      const inst = byName.get(p.name) ?? null;
      seen.add(p.name);
      out.push({
        key: p.name,
        label: p.label,
        group: (p.group as AtlasItem['group']) || 'hosted',
        preset: p,
        installed: inst,
        connected: inst
          ? p.name === 'local'
            ? localStatus
              ? localStatus.server_running || localStatus.models.some((m) => m.downloaded)
              : true
            : inst.auth_kind !== 'none'
          : false,
        isDefault: inst?.is_default ?? false,
        kind: inst?.kind ?? p.kind,
        baseUrl: inst?.base_url ?? p.base_url,
        envHint: p.env_hint,
        keyUrl: p.key_url,
      });
    }

    for (const inst of providers) {
      if (seen.has(inst.name)) continue;
      out.push({
        key: inst.name,
        label: inst.name,
        group: 'custom',
        preset: null,
        installed: inst,
        connected: inst.auth_kind !== 'none',
        isDefault: inst.is_default,
        kind: inst.kind,
        baseUrl: inst.base_url,
        envHint: null,
        keyUrl: null,
      });
    }

    return out;
  });

  const installedItems = $derived(
    atlasItems
      .filter((item) => item.installed !== null)
      .sort((a, b) => Number(b.isDefault) - Number(a.isDefault) || a.label.localeCompare(b.label)),
  );

  const defaultItem = $derived(installedItems.find((i) => i.isDefault) ?? installedItems[0] ?? null);

  const filteredItems = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return atlasItems.filter((item) => {
      if (cat === 'active' && !item.installed) return false;
      if (cat !== 'all' && cat !== 'active' && item.group !== cat) return false;
      if (!q) return true;
      return (
        item.label.toLowerCase().includes(q) ||
        item.key.toLowerCase().includes(q) ||
        item.baseUrl.toLowerCase().includes(q) ||
        (item.envHint ?? '').toLowerCase().includes(q) ||
        (item.installed?.model ?? '').toLowerCase().includes(q) ||
        item.group.toLowerCase().includes(q)
      );
    });
  });

  const groupedFiltered = $derived.by(() => {
    if (query.trim() || cat !== 'all') return null;
    return GROUPS.map((g) => ({
      ...g,
      items: filteredItems.filter((i) => i.group === g.key),
    })).filter((g) => g.items.length > 0);
  });

  const selectedItem = $derived(
    selectedKey && selectedKey !== '__custom__'
      ? atlasItems.find((i) => i.key === selectedKey) ?? null
      : null,
  );

  const isCustomBuilder = $derived(selectedKey === '__custom__');

  function slugFromUrl(url: string): string {
    const host = url.trim().replace(/^https?:\/\//, '').split('/')[0] ?? '';
    const label = host
      .split('.')
      .map((l) => l.toLowerCase())
      .find((l) => l && !['api', 'www', 'chat', 'inference', 'v1'].includes(l));
    return (label ?? '')
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 32);
  }

  $effect(() => {
    if (!isCustomBuilder || setupNameTouched) return;
    const suggested = slugFromUrl(setupBaseUrl);
    if (suggested && suggested !== setupName) setupName = suggested;
  });

  const existingNames = $derived(providers.map((p) => p.name));
  const customNameTaken = $derived(
    isCustomBuilder && existingNames.some((n) => n.toLowerCase() === setupName.trim().toLowerCase()),
  );
  const customNameInvalid = $derived(
    isCustomBuilder && setupName.trim() !== '' && !/^[a-z0-9._-]+$/.test(setupName.trim()),
  );
  const customEndpointMissing = $derived(
    isCustomBuilder && !/^https?:\/\/[^/]/.test(setupBaseUrl.trim()),
  );

  const canFinishSetup = $derived.by(() => {
    if (isCustomBuilder) {
      return (
        setupModel.trim().length > 0 &&
        setupName.trim().length > 0 &&
        !customNameTaken &&
        !customNameInvalid &&
        !customEndpointMissing
      );
    }
    const preset = selectedItem?.preset;
    if (!preset) return false;
    if (preset.auth === 'agent') return true;
    return setupModel.trim().length > 0 && setupBaseUrl.trim().length > 0;
  });

  function selectKey(key: string): void {
    if (selectedKey === key) return;
    selectedKey = key;
    confirmRemove = null;
    confirmDeleteModel = null;
    error = null;
    localError = null;
    localNotice = null;
    modelFilter = '';
    resetSetupState(key);
    if (key === 'local' && inTauri) void refreshLocalStatus();
  }

  function resetSetupState(key: string): void {
    setupKey = '';
    setupShowKey = false;
    setupModels = [];
    setupScanning = false;
    setupScanError = null;
    setupBusy = false;
    setupError = null;
    setupShowAdvanced = false;
    setupUseNow = true;
    setupName = '';
    setupNameTouched = false;

    if (key === '__custom__') {
      setupKind = 'openai_compat';
      setupBaseUrl = '';
      setupModel = '';
      return;
    }

    const item = atlasItems.find((i) => i.key === key);
    if (item?.preset) {
      setupKind = item.preset.kind === 'anthropic' ? 'anthropic' : 'openai_compat';
      setupBaseUrl = item.preset.base_url;
      setupModel = item.preset.model;
    } else {
      setupKind = 'openai_compat';
      setupBaseUrl = '';
      setupModel = '';
    }
  }

  function ensureSelection(): void {
    if (selectedKey === '__custom__') return;
    if (selectedKey && atlasItems.some((i) => i.key === selectedKey)) return;
    const target =
      providers.find((p) => p.is_default)?.name ??
      providers[0]?.name ??
      presets.find((p) => p.group !== 'custom')?.name ??
      null;
    if (target) {
      selectedKey = target;
      resetSetupState(target);
    }
  }

  async function refresh(): Promise<void> {
    if (!inTauri) return;
    const m = await import('../ipc');
    providers = await m.providersList().catch(() => []);
    presets = await m.providerPresets().catch(() => []);
    if (providers.some((p) => p.kind === 'acp')) {
      agStatus = await m.antigravityStatus().catch(() => null);
    } else {
      agStatus = null;
    }
    if (selectedKey === 'local' || providers.some((p) => p.name === 'local') || !localStatus) {
      localStatus = await m.localRuntimeStatus().catch(() => localStatus);
    }
    ensureSelection();
    void session.refreshModels();
  }

  onMount(async () => {
    try {
      const m = await import('../ipc');
      await m.appInfo();
      inTauri = true;
      checkedEnv = true;
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
      localUnlisten = await listen<{
        kind: string;
        modelId?: string;
        downloadedMb?: number;
        totalMb?: number | null;
        pct?: number;
        backend?: string;
        message?: string;
      }>('local-runtime-event', (e) => {
        const p = e.payload;
        if (p.kind === 'model_download' && p.modelId) {
          localDlProgress = {
            modelId: p.modelId,
            downloadedMb: p.downloadedMb ?? 0,
            totalMb: p.totalMb ?? null,
            pct: p.pct ?? 0,
          };
        } else if (p.kind === 'runtime_download') {
          localRuntimeDl = {
            backend: p.backend ?? '',
            downloadedMb: p.downloadedMb ?? 0,
            totalMb: p.totalMb ?? null,
          };
        } else if (p.kind === 'fallback' && p.message) {
          localNotice = p.message;
        } else if (p.kind === 'server_ready') {
          localRuntimeDl = null;
          void refreshLocalStatus();
        }
      });
      unlisten = await listen<{ type: string; ok: boolean; error?: string }>('auth-event', (e) => {
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
      });
    } catch {
      checkedEnv = true;
    }
  });

  onDestroy(() => {
    unlisten?.();
    agUnlisten?.();
    localUnlisten?.();
    if (confirmTimer) clearTimeout(confirmTimer);
    if (confirmDeleteTimer) clearTimeout(confirmDeleteTimer);
  });

  function onGlobalKeydown(e: KeyboardEvent): void {
    if (!active || !inTauri) return;
    const el = e.target instanceof Element ? e.target : null;
    const typing = !!el?.closest('input, textarea, select, [contenteditable="true"]');
    if (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      searchEl?.focus();
    }
  }

  function groupLabel(key: AtlasItem['group']): string {
    return GROUPS.find((g) => g.key === key)?.label ?? key;
  }

  function protocolLabel(kind: string): string {
    if (kind === 'acp') return 'Google Antigravity';
    if (kind === 'anthropic') return 'Anthropic Messages';
    return 'OpenAI Chat';
  }

  function authBadgeLabel(item: AtlasItem): string {
    if (item.key === 'local') {
      if (localStatus?.server_running) return 'running · on device';
      if (item.installed || localStatus?.models.some((m) => m.downloaded)) return 'ready · on device';
      return 'zero-setup · on device';
    }
    const p = item.installed;
    if (!p) {
      const auth = item.preset?.auth;
      if (auth === 'none') return 'local · no key';
      if (auth === 'agent') return 'google sign-in';
      if (auth === 'oauth') return 'oauth or key';
      return 'not configured';
    }
    if (p.kind === 'acp') return p.auth_kind === 'oauth' ? 'google sign-in' : 'sign-in needed';
    if (p.auth_kind === 'oauth') return 'subscription';
    if (p.auth_kind === 'api_key') return 'api key';
    if (p.auth_kind === 'env') return 'env key';
    return item.preset?.auth === 'none' ? 'local ready' : 'key needed';
  }

  function rowHint(item: AtlasItem): string {
    if (item.key === 'local') {
      if (localStatus?.server_running && localStatus.active_model) {
        return `${localStatus.active_model} · live`;
      }
      if (item.installed?.model) return `${item.installed.model} · stopped`;
      if (localStatus?.specs?.gpu_name) return localStatus.specs.gpu_name;
      return 'On-device · zero setup';
    }
    if (item.installed) {
      if (item.installed.model) return item.installed.model;
      if (item.installed.kind === 'acp') {
        return agStatus?.signed_in ? `${agStatus.models} models` : 'Google agent';
      }
      return item.baseUrl;
    }
    if (item.preset?.auth === 'none') return item.baseUrl.replace(/^https?:\/\//, '');
    if (item.preset?.auth === 'agent') return 'Google account';
    if (item.preset?.auth === 'oauth') return 'OAuth · API key';
    return item.envHint ?? 'API key';
  }

  async function copyEndpoint(url: string): Promise<void> {
    const ok = await navigator.clipboard
      .writeText(url)
      .then(() => true)
      .catch(() => false);
    if (ok) {
      copiedUrl = true;
      setTimeout(() => (copiedUrl = false), 1500);
    }
  }

  async function scanSetupModels(): Promise<void> {
    const kind = isCustomBuilder ? setupKind : selectedItem?.preset?.kind ?? 'openai_compat';
    if (!setupBaseUrl.trim()) return;
    setupScanning = true;
    setupScanError = null;
    setupModels = [];
    try {
      const m = await import('../ipc');
      const list = await m.providerPreviewModels(kind, setupBaseUrl.trim(), setupKey);
      setupModels = list
        .filter((x) => !/embed|bge-|e5-|gte-|voyage|paraphrase|rerank|:batch/i.test(x.id))
        .sort((a, b) => a.id.localeCompare(b.id));
      if (setupModels.length === 0) {
        setupScanError = 'Endpoint returned no chat models.';
      } else if (!setupModels.some((x) => x.id === setupModel)) {
        const preferred =
          setupModels.find((x) => x.id === selectedItem?.preset?.model) ??
          setupModels.find((x) => /sonnet|gpt-5|pro|large|chat/i.test(x.id)) ??
          setupModels[0];
        if (preferred) setupModel = preferred.id;
      }
    } catch (e) {
      setupScanError = e instanceof Error ? e.message : String(e);
    } finally {
      setupScanning = false;
    }
  }

  async function finishSetup(mode: 'default' | 'oauth' = 'default'): Promise<void> {
    setupBusy = true;
    setupError = null;
    try {
      const m = await import('../ipc');
      const provider = isCustomBuilder ? setupName.trim() : selectedItem?.preset?.name ?? '';
      if (!provider) return;
      const preset = selectedItem?.preset;
      const kind = isCustomBuilder ? setupKind : preset?.kind ?? 'openai_compat';
      const baseUrl = (isCustomBuilder ? setupBaseUrl : setupBaseUrl || preset?.base_url || '').trim();
      const model = (preset?.auth === 'agent' ? preset.model : setupModel).trim();

      await m.providerUpsert(provider, kind, baseUrl, model);
      if ((isCustomBuilder || preset?.auth === 'api_key' || preset?.auth === 'oauth') && setupKey.trim()) {
        await m.authSetKey(provider, setupKey.trim());
      }

      selectedKey = provider;
      justAdded = provider;
      setTimeout(() => (justAdded = null), 2400);

      await refresh();

      if (setupUseNow && model) {
        await session.setModel(model, provider).catch(() => {});
        await refresh();
      } else if (setupUseNow && kind === 'acp') {
        await m.sessionSetProvider(provider, null).catch(() => {});
        session.providerName = provider;
        session.provider = provider;
        await refresh();
      }

      if (mode === 'oauth') {
        if (provider === 'claude') await connectClaude();
        else if (provider === 'nous') await connectNous();
      }
    } catch (e) {
      setupError = e instanceof Error ? e.message : String(e);
    } finally {
      setupBusy = false;
    }
  }

  async function activateProvider(p: ProviderInfo): Promise<void> {
    activateBusy = p.name;
    error = null;
    try {
      if (p.model) {
        await session.setModel(p.model, p.name);
      } else {
        const m = await import('../ipc');
        await m.sessionSetProvider(p.name, null);
        session.providerName = p.name;
        session.provider = p.name;
      }
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      activateBusy = null;
    }
  }

  async function loadProviderModels(name: string): Promise<void> {
    modelBusy = name;
    error = null;
    try {
      const m = await import('../ipc');
      const list = await m.modelsList(name);
      const filtered = list
        .filter((x) => !/embed|bge-|e5-|gte-|voyage|paraphrase|rerank|:batch/i.test(x.id))
        .sort((a, b) => a.id.localeCompare(b.id));
      modelLists = { ...modelLists, [name]: filtered };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      modelBusy = null;
    }
  }

  async function pickModel(provider: string, model: string): Promise<void> {
    error = null;
    try {
      await session.setModel(model, provider);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
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

  async function disconnectCredentials(p: ProviderInfo): Promise<void> {
    logoutBusy = p.name;
    error = null;
    try {
      const m = await import('../ipc');
      if (p.kind === 'acp') {
        agStatus = await m.antigravityLogout();
      } else {
        await m.authLogout(p.name);
      }
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      logoutBusy = null;
    }
  }

  async function removeProvider(p: ProviderInfo): Promise<void> {
    if (confirmRemove !== p.name) {
      confirmRemove = p.name;
      if (confirmTimer) clearTimeout(confirmTimer);
      confirmTimer = setTimeout(() => (confirmRemove = null), 8000);
      return;
    }
    confirmRemove = null;
    if (confirmTimer) clearTimeout(confirmTimer);
    const m = await import('../ipc');
    error = null;
    if (p.name === 'local') {
      localError = null;
      localNotice = null;
    }
    let ok = false;
    try {
      if (p.kind === 'acp') await m.antigravityLogout().catch(() => {});
      await m.providerRemove(p.name);
      ok = true;
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (p.name === 'local') localError = msg;
      else error = msg;
    }
    await refresh();
    if (p.name === 'local' && ok) {
      localNotice = 'Built-in Local disabled and local server stopped.';
    }
    if (selectedItem?.preset) {
      resetSetupState(p.name);
    } else {
      selectedKey = null;
      ensureSelection();
    }
  }

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

  const activeModelMeta = $derived.by(() => {
    const p = selectedItem?.installed;
    if (!p || !p.model) return null;
    const list = modelLists[p.name] ?? [];
    return (
      list.find((x) => x.id === p.model) ??
      session.models.find((x) => x.id === p.model && (!x.provider || x.provider === p.name)) ??
      null
    );
  });

  const filteredLoadedModels = $derived.by(() => {
    const p = selectedItem?.installed;
    if (!p) return [] as ProviderModel[];
    const list = modelLists[p.name] ?? [];
    const q = modelFilter.trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (m) => m.id.toLowerCase().includes(q) || (m.name ?? '').toLowerCase().includes(q),
    );
  });

  const sortedLocalModels = $derived.by<LocalModelStatus[]>(() => {
    const list = localStatus?.models ?? [];
    const rank = (fit: string) =>
      fit === 'gpu' ? 0 : fit === 'hybrid' ? 1 : fit === 'cpu' ? 2 : 3;
    return [...list].sort(
      (a, b) =>
        Number(b.active) - Number(a.active) ||
        Number(b.recommended) - Number(a.recommended) ||
        Number(b.downloaded) - Number(a.downloaded) ||
        rank(a.fit) - rank(b.fit) ||
        a.size_mb - b.size_mb,
    );
  });

  function fmtGb(mb: number): string {
    if (mb >= 1024) {
      const gb = mb / 1024;
      return `${gb >= 10 ? gb.toFixed(0) : gb.toFixed(1)} GB`;
    }
    return `${mb} MB`;
  }

  function backendLabel(b: string): string {
    if (b === 'cuda') return 'CUDA (NVIDIA GPU)';
    if (b === 'vulkan') return 'Vulkan (GPU)';
    if (b === 'metal') return 'Metal (Apple GPU)';
    if (b === 'cpu') return 'CPU Only (Safe Mode)';
    return b.toUpperCase();
  }

  function fitBadgeLabel(m: LocalModelStatus, specs: HardwareSpecs): string {
    if (m.fit === 'gpu') return specs.unified_memory ? 'Unified GPU' : 'Full GPU';
    if (m.fit === 'hybrid') return 'GPU + RAM';
    if (m.fit === 'cpu') return 'CPU RAM';
    return 'Exceeds Memory';
  }

  async function refreshLocalStatus(explicit = false): Promise<void> {
    if (!inTauri) return;
    localLoading = true;
    if (explicit) {
      localError = null;
      localNotice = null;
    }
    try {
      const m = await import('../ipc');
      localStatus = explicit
        ? await m.localRuntimeRefresh()
        : await m.localRuntimeStatus();
      if (explicit) {
        await refresh();
        const dlCount = localStatus.models.filter((x) => x.downloaded).length;
        localNotice = `Refreshed · ${dlCount} model${dlCount === 1 ? '' : 's'} downloaded · ${fmtGb(localStatus.specs.disk_free_mb)} free.`;
      }
    } catch (e) {
      localError = e instanceof Error ? e.message : String(e);
    } finally {
      localLoading = false;
    }
  }

  async function setLocalPrefs(backend: string | null, ctx: number | null): Promise<void> {
    localBusy = 'prefs';
    localError = null;
    try {
      const m = await import('../ipc');
      localStatus = await m.localRuntimeSetPrefs(backend, ctx);
    } catch (e) {
      localError = e instanceof Error ? e.message : String(e);
    } finally {
      localBusy = null;
    }
  }

  async function installLocalRuntime(): Promise<void> {
    localBusy = 'install';
    localError = null;
    localRuntimeDl = null;
    try {
      const m = await import('../ipc');
      localStatus = await m.localRuntimeInstall(null);
    } catch (e) {
      localError = e instanceof Error ? e.message : String(e);
    } finally {
      localBusy = null;
      localRuntimeDl = null;
    }
  }

  async function downloadLocalModel(modelId: string): Promise<void> {
    localBusy = modelId;
    localError = null;
    localDlProgress = { modelId, downloadedMb: 0, totalMb: null, pct: 0 };
    try {
      const m = await import('../ipc');
      localStatus = await m.localModelDownload(modelId);
      await refresh();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (!/cancelled/i.test(msg)) localError = msg;
      await refreshLocalStatus();
    } finally {
      localBusy = null;
      localDlProgress = null;
    }
  }

  async function cancelLocalDownload(): Promise<void> {
    const m = await import('../ipc');
    await m.localModelDownloadCancel().catch(() => {});
  }

  async function deleteLocalModel(modelId: string, label?: string, skipConfirm = false): Promise<void> {
    if (!skipConfirm && confirmDeleteModel !== modelId) {
      confirmDeleteModel = modelId;
      if (confirmDeleteTimer) clearTimeout(confirmDeleteTimer);
      confirmDeleteTimer = setTimeout(() => (confirmDeleteModel = null), 8000);
      return;
    }
    confirmDeleteModel = null;
    if (confirmDeleteTimer) clearTimeout(confirmDeleteTimer);
    localBusy = `del:${modelId}`;
    localError = null;
    localNotice = null;
    try {
      const m = await import('../ipc');
      localStatus = await m.localModelDelete(modelId);
      await refresh();
      localNotice = skipConfirm
        ? `Discarded partial download for ${label ?? modelId}.`
        : `Uninstalled ${label ?? modelId}.`;
    } catch (e) {
      localError = e instanceof Error ? e.message : String(e);
    } finally {
      localBusy = null;
    }
  }

  async function startLocalModel(modelId: string): Promise<void> {
    localBusy = `start:${modelId}`;
    localError = null;
    localNotice = null;
    try {
      const m = await import('../ipc');
      localStatus = await m.localModelStart(modelId, true);
      justAdded = 'local';
      setTimeout(() => (justAdded = null), 2400);
      await refresh();
      if (session.providerName !== 'local' || session.model !== modelId) {
        await session.setModel(modelId, 'local');
      } else {
        await m.sessionSetProvider('local', modelId).catch(() => {});
      }
      await refresh();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (!/cancelled/i.test(msg)) localError = msg;
      await refreshLocalStatus();
    } finally {
      if (localBusy === `start:${modelId}`) localBusy = null;
      localRuntimeDl = null;
    }
  }

  async function stopLocalServer(): Promise<void> {
    localBusy = 'stop';
    localError = null;
    localNotice = null;
    confirmDeleteModel = null;
    confirmRemove = null;
    try {
      const m = await import('../ipc');
      localStatus = await m.localRuntimeStop();
      await refresh();
      localNotice = 'Local server stopped.';
    } catch (e) {
      localError = e instanceof Error ? e.message : String(e);
    } finally {
      if (localBusy === 'stop') localBusy = null;
    }
  }

  async function copyModelsDir(dir: string): Promise<void> {
    const ok = await navigator.clipboard.writeText(dir).then(() => true).catch(() => false);
    if (ok) {
      localModelsCopied = true;
      setTimeout(() => (localModelsCopied = false), 1500);
    }
  }

  function formatCtx(tokens: number | null | undefined): string | null {
    if (!tokens || tokens <= 0) return null;
    if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(tokens % 1_000_000 === 0 ? 0 : 1)}M ctx`;
    if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k ctx`;
    return `${tokens} ctx`;
  }
</script>

<svelte:window onkeydown={onGlobalKeydown} />

<div class="prov-wrap">
  <header class="prov-top">
    <div class="prov-heading">
      <h2 class="group-title">Providers</h2>
      <p class="group-hint">
        One switchboard for every model endpoint. Keys stay in your OS keychain.
      </p>
    </div>

    {#if inTauri}
      <div class="route-telemetry">
        {#if defaultItem?.installed}
          <button
            class="active-route"
            onclick={() => selectKey(defaultItem.key)}
            title="Jump to the active default provider"
          >
            <span class="beacon" class:ok={defaultItem.connected} class:warn={!defaultItem.connected}></span>
            <span class="route-label mono">ACTIVE</span>
            <span class="route-mark" aria-hidden="true">
              <ProviderLogo provider={defaultItem.key} label={defaultItem.label} size={12} fallbackFontSize={13} />
            </span>
            <span class="route-name">{defaultItem.label}</span>
            {#if defaultItem.installed.model}
              <span class="route-sep">/</span>
              <span class="route-model mono">{defaultItem.installed.model}</span>
            {/if}
          </button>
        {:else}
          <div class="active-route empty">
            <span class="beacon"></span>
            <span class="route-label mono">NO ACTIVE ROUTE</span>
          </div>
        {/if}
      </div>
    {/if}
  </header>

  {#if !checkedEnv}
    <p class="muted">Checking provider configuration…</p>
  {:else if !inTauri}
    <div class="offline-card">
      <Icon name="key" size={16} />
      <span>Provider management requires the desktop app.</span>
    </div>
  {:else}
    {#if installedItems.length > 0}
      <div class="route-strip" role="navigation" aria-label="Configured providers">
        <span class="strip-caption mono">CONFIGURED</span>
        <div class="strip-chips">
          {#each installedItems as item (item.key)}
            <button
              class="route-chip"
              class:selected={selectedKey === item.key}
              class:is-default={item.isDefault}
              onclick={() => selectKey(item.key)}
              title={item.installed?.model || item.baseUrl}
            >
              <span
                class="beacon sm"
                class:ok={item.connected}
                class:warn={!item.connected}
              ></span>
              <span class="chip-mark" aria-hidden="true">
                <ProviderLogo provider={item.key} label={item.label} size={12} fallbackFontSize={13} />
              </span>
              <span class="chip-name">{item.label}</span>
              {#if item.isDefault}
                <span class="chip-tag mono">DEFAULT</span>
              {/if}
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <div class="switchboard">
      <!-- LEFT: Unified Searchable Directory -->
      <aside class="atlas" aria-label="Provider directory">
        <div class="atlas-search">
          <Icon name="search" size={13} />
          <input
            bind:this={searchEl}
            class="atlas-input"
            type="text"
            placeholder="Find provider, URL, key… (/)"
            aria-label="Find provider"
            bind:value={query}
          />
          {#if query}
            <button
              class="atlas-clear"
              onclick={() => (query = '')}
              aria-label="Clear search"
              title="Clear search"
            >
              <Icon name="x" size={11} />
            </button>
          {/if}
        </div>

        <div class="atlas-cats" role="tablist" aria-label="Provider categories">
          <button
            class="cat-pill"
            class:on={cat === 'all'}
            role="tab"
            aria-selected={cat === 'all'}
            onclick={() => (cat = 'all')}
          >
            All <span class="cat-n mono">{atlasItems.length}</span>
          </button>
          {#if installedItems.length > 0}
            <button
              class="cat-pill"
              class:on={cat === 'active'}
              role="tab"
              aria-selected={cat === 'active'}
              onclick={() => (cat = 'active')}
            >
              Live <span class="cat-n mono">{installedItems.length}</span>
            </button>
          {/if}
          {#each GROUPS as g (g.key)}
            {#if g.key !== 'custom' || atlasItems.some((i) => i.group === 'custom')}
              <button
                class="cat-pill"
                class:on={cat === g.key}
                role="tab"
                aria-selected={cat === g.key}
                onclick={() => (cat = g.key)}
              >
                {g.short}
              </button>
            {/if}
          {/each}
        </div>

        <div class="atlas-list" role="listbox" aria-label="Providers">
          {#if filteredItems.length === 0}
            <div class="atlas-empty">
              <p>No match for “{query}”.</p>
              <button class="btn sm ghost" onclick={() => { query = ''; cat = 'all'; }}>
                Reset filter
              </button>
            </div>
          {:else if groupedFiltered}
            {#each groupedFiltered as group (group.key)}
              <div class="atlas-group-label mono">{group.label}</div>
              {#each group.items as item (item.key)}
                <button
                  class="atlas-row"
                  class:selected={selectedKey === item.key}
                  class:installed={!!item.installed}
                  class:is-default={item.isDefault}
                  role="option"
                  aria-selected={selectedKey === item.key}
                  onclick={() => selectKey(item.key)}
                >
                  <span class="row-mark" class:live={item.connected} aria-hidden="true">
                    <ProviderLogo provider={item.key} label={item.label} size={15} fallbackFontSize={16} />
                  </span>
                  <span class="row-copy">
                    <span class="row-name">{item.label}</span>
                    <span class="row-sub mono">{rowHint(item)}</span>
                  </span>
                  <span class="row-status">
                    {#if item.isDefault}
                      <span class="row-tag mono">LIVE</span>
                    {:else if item.connected}
                      <span class="beacon sm ok" title="Connected"></span>
                    {:else if item.installed}
                      <span class="beacon sm warn" title="Needs credentials"></span>
                    {:else}
                      <span class="row-arrow" aria-hidden="true">
                        <Icon name="chevron-right" size={12} />
                      </span>
                    {/if}
                  </span>
                </button>
              {/each}
            {/each}
          {:else}
            {#each filteredItems as item (item.key)}
              <button
                class="atlas-row"
                class:selected={selectedKey === item.key}
                class:installed={!!item.installed}
                class:is-default={item.isDefault}
                role="option"
                aria-selected={selectedKey === item.key}
                onclick={() => selectKey(item.key)}
              >
                <span class="row-mark" class:live={item.connected} aria-hidden="true">
                  <ProviderLogo provider={item.key} label={item.label} size={15} fallbackFontSize={16} />
                </span>
                <span class="row-copy">
                  <span class="row-name">{item.label}</span>
                  <span class="row-sub mono">{rowHint(item)}</span>
                </span>
                <span class="row-status">
                  {#if item.isDefault}
                    <span class="row-tag mono">LIVE</span>
                  {:else if item.connected}
                    <span class="beacon sm ok" title="Connected"></span>
                  {:else if item.installed}
                    <span class="beacon sm warn" title="Needs credentials"></span>
                  {:else}
                    <span class="row-arrow" aria-hidden="true">
                      <Icon name="chevron-right" size={12} />
                    </span>
                  {/if}
                </span>
              </button>
            {/each}
          {/if}
        </div>

        <div class="atlas-foot">
          <button
            class="custom-trigger"
            class:selected={isCustomBuilder}
            onclick={() => selectKey('__custom__')}
          >
            <span class="custom-plus">
              <Icon name="plus" size={12} />
            </span>
            <span class="custom-copy">
              <span class="custom-title">Custom endpoint</span>
              <span class="custom-sub mono">Any OpenAI / Anthropic URL</span>
            </span>
          </button>
        </div>
      </aside>

      <!-- RIGHT: Dedicated Configuration Stage -->
      <section class="stage" aria-live="polite">
        {#if isCustomBuilder}
          <div class="stage-head">
            <div class="stage-emblem custom" aria-hidden="true">
              <Icon name="plug" size={20} />
            </div>
            <div class="stage-title-wrap">
              <div class="stage-title-row">
                <h3 class="stage-title">Custom Endpoint</h3>
                <span class="state-pill mono">UNCATALOGUED API</span>
              </div>
              <div class="stage-meta-row">
                <span class="meta-chip mono">Custom</span>
                <span class="meta-chip mono">{protocolLabel(setupKind)}</span>
              </div>
            </div>
          </div>

          <div class="stage-body">
            <p class="stage-intro">
              Connect any self-hosted gateway, corporate proxy, or boutique inference endpoint that speaks OpenAI Chat Completions or Anthropic Messages.
            </p>

            <div class="form-grid">
              <label class="fld">
                <span class="fld-label">Provider name</span>
                <input
                  class="input mono"
                  type="text"
                  placeholder="silkcompute"
                  spellcheck="false"
                  bind:value={setupName}
                  oninput={() => (setupNameTouched = true)}
                />
                <span class="fld-hint">
                  {#if customNameTaken}
                    <span class="bad">Already in use — pick a fresh slug.</span>
                  {:else if customNameInvalid}
                    <span class="bad">Lowercase letters, digits, dots, dashes only.</span>
                  {:else}
                    Shown in /model and the provider switchboard.
                  {/if}
                </span>
              </label>

              <div class="fld">
                <span class="fld-label">Wire format</span>
                <div class="seg-group" role="group" aria-label="Wire format">
                  {#each KINDS as k (k.id)}
                    <button
                      class="seg-btn"
                      class:on={setupKind === k.id}
                      aria-pressed={setupKind === k.id}
                      onclick={() => (setupKind = k.id)}
                    >
                      {k.label}
                    </button>
                  {/each}
                </div>
              </div>

              <label class="fld fld-full">
                <span class="fld-label">Base URL</span>
                <input
                  class="input mono"
                  type="text"
                  placeholder="https://api.example.com/v1"
                  spellcheck="false"
                  bind:value={setupBaseUrl}
                />
                <span class="fld-hint">
                  {#if customEndpointMissing && setupBaseUrl.trim()}
                    <span class="bad">Must start with https:// or http:// and include a host.</span>
                  {:else}
                    Stop at the version segment — the app appends /chat/completions or /messages automatically.
                  {/if}
                </span>
              </label>

              <div class="fld fld-full">
                <span class="fld-label">API Key (optional)</span>
                <div class="key-input-row">
                  <div class="secret-wrap">
                    <input
                      class="input mono"
                      type={setupShowKey ? 'text' : 'password'}
                      placeholder="sk-…"
                      bind:value={setupKey}
                    />
                    <button
                      type="button"
                      class="eye-btn"
                      onclick={() => (setupShowKey = !setupShowKey)}
                      aria-label={setupShowKey ? 'Hide key' : 'Show key'}
                    >
                      <Icon name={setupShowKey ? 'eye-off' : 'eye'} size={13} />
                    </button>
                  </div>
                  <button
                    class="btn"
                    onclick={scanSetupModels}
                    disabled={setupScanning || !setupBaseUrl.trim()}
                  >
                    {setupScanning ? 'Testing…' : 'Test & list models'}
                  </button>
                </div>
                {#if setupScanError}
                  <span class="fld-hint bad">{setupScanError}</span>
                {:else if setupModels.length > 0}
                  <span class="fld-hint good">
                    Endpoint reachable — {setupModels.length} model{setupModels.length === 1 ? '' : 's'} discovered.
                  </span>
                {:else}
                  <span class="fld-hint">Leave blank if your endpoint does not require authentication.</span>
                {/if}
              </div>

              <label class="fld fld-full">
                <span class="fld-label">
                  Model ID
                  {#if setupModels.length > 0}
                    <span class="fld-badge mono">{setupModels.length} found</span>
                  {/if}
                </span>
                {#if setupModels.length > 0}
                  <select class="input mono" bind:value={setupModel}>
                    {#each setupModels as m (m.id)}
                      <option value={m.id}>{m.id}{m.name && m.name !== m.id ? ` · ${m.name}` : ''}</option>
                    {/each}
                  </select>
                {:else}
                  <input
                    class="input mono"
                    type="text"
                    placeholder="e.g. llama-3.3-70b-instruct"
                    spellcheck="false"
                    bind:value={setupModel}
                  />
                {/if}
              </label>
            </div>

            <label class="check-row">
              <input type="checkbox" bind:checked={setupUseNow} />
              <span>Make this the active provider immediately</span>
            </label>

            {#if setupError}
              <p class="error-banner">{setupError}</p>
            {/if}

            <div class="stage-actions-foot">
              <button
                class="btn primary"
                onclick={() => void finishSetup('default')}
                disabled={!canFinishSetup || setupBusy}
              >
                {setupBusy ? 'Adding…' : 'Add Custom Endpoint'}
              </button>
              {#if customPreset?.label}
                <span class="foot-note mono">Stored locally in config + keychain</span>
              {/if}
            </div>
          </div>
        {:else if selectedItem}
          {@const item = selectedItem}
          {@const p = item.installed}
          {@const preset = item.preset}
          {@const isAcp = item.kind === 'acp'}
          {@const isBuiltInLocal = item.key === 'local'}
          {@const isFresh = justAdded === item.key}
          {@const liveLocalUrl = isBuiltInLocal
            ? `http://127.0.0.1:${localStatus?.server_port ?? 11435}/v1`
            : item.baseUrl}

          <div class="stage-head" class:fresh={isFresh}>
            <div
              class="stage-emblem"
              class:live={isBuiltInLocal ? (localStatus?.server_running || !!p) : item.connected}
              class:armed={!isBuiltInLocal && !!p && !item.connected}
            >
              <ProviderLogo provider={item.key} label={item.label} size={24} fallbackFontSize={28} />
              <span
                class="emblem-dot"
                class:ok={isBuiltInLocal ? (localStatus?.server_running || !!p) : item.connected}
                class:warn={!isBuiltInLocal && !!p && !item.connected}
              ></span>
            </div>

            <div class="stage-title-wrap">
              <div class="stage-title-row">
                <h3 class="stage-title">{item.label}</h3>
                {#if item.label !== item.key}
                  <span class="slug-tag mono">{item.key}</span>
                {/if}
                <span
                  class="state-pill mono"
                  class:ok={isBuiltInLocal ? (localStatus?.server_running || !!p) : item.connected}
                  class:warn={!isBuiltInLocal && !!p && !item.connected}
                >
                  {authBadgeLabel(item)}
                </span>
              </div>
              <div class="stage-meta-row">
                <span class="meta-chip mono">{groupLabel(item.group)}</span>
                <span class="meta-chip mono">
                  {isBuiltInLocal ? 'Managed llama.cpp' : protocolLabel(item.kind)}
                </span>
                {#if item.keyUrl}
                  <a
                    class="key-link mono"
                    href={item.keyUrl}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {preset?.auth === 'agent' ? 'Plans' : 'Get API key'}
                    <Icon name="arrow-up-right" size={11} />
                  </a>
                {/if}
              </div>
            </div>

            {#if p || (isBuiltInLocal && (localStatus?.server_running || localBusy?.startsWith('start:')))}
              <div class="stage-head-cta">
                {#if isBuiltInLocal && (localStatus?.server_running || localBusy?.startsWith('start:'))}
                  <button
                    class="btn sm ghost"
                    onclick={() => void stopLocalServer()}
                    disabled={localBusy === 'stop'}
                  >
                    <Icon name="stop" size={11} />
                    {localBusy === 'stop'
                      ? 'Stopping…'
                      : localStatus?.server_running
                        ? 'Stop server'
                        : 'Cancel start'}
                  </button>
                {/if}
                {#if p}
                  {#if item.isDefault}
                    <span class="active-badge mono">
                      <Icon name="check" size={11} />
                      Active route
                    </span>
                  {:else}
                    <button
                      class="btn primary sm"
                      onclick={() => void activateProvider(p)}
                      disabled={activateBusy === p.name}
                    >
                      {activateBusy === p.name ? 'Switching…' : 'Set active'}
                    </button>
                  {/if}
                {/if}
              </div>
            {/if}
          </div>

          {#if liveLocalUrl}
            <div class="endpoint-bar">
              <span class="endpoint-label mono">ENDPOINT</span>
              <code class="endpoint-url mono" title={liveLocalUrl}>{liveLocalUrl}</code>
              {#if isBuiltInLocal && localStatus}
                <span class="env-chip mono">
                  {localStatus.server_running
                    ? (localStatus.active_backend_used ?? 'running')
                    : localStatus.specs.recommended_backend}
                </span>
              {:else if item.envHint}
                <span class="env-chip mono" title="Fallback environment variable">{item.envHint}</span>
              {/if}
              {#if liveLocalUrl.startsWith('http')}
                <button
                  class="copy-mini"
                  onclick={() => void copyEndpoint(liveLocalUrl)}
                  title="Copy base URL"
                  aria-label="Copy base URL"
                >
                  <Icon name={copiedUrl ? 'check' : 'copy'} size={11} />
                </button>
              {/if}
            </div>
          {/if}

          <div class="stage-body">
            {#if isBuiltInLocal}
              {#if !localStatus}
                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">BUILT-IN LOCAL RUNTIME</span>
                  </div>
                  <p class="deck-copy">
                    {localLoading ? 'Detecting your CPU, GPU, and memory…' : 'Loading hardware profile…'}
                  </p>
                </div>
              {:else}
                {@const specs = localStatus.specs}
                {@const activeDownloadingId = localStatus.downloading_model ?? localDlProgress?.modelId ?? null}

                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">YOUR PC HARDWARE</span>
                    <div class="deck-head-right">
                      <span class="deck-sub mono">{specs.os} &middot; {specs.arch}</span>
                      <span class="spec-chip mono">{fmtGb(specs.disk_free_mb)} free</span>
                    </div>
                  </div>

                  <div class="local-spec-grid">
                    <div class="local-spec-cell">
                      <span class="spec-k mono">PROCESSOR</span>
                      <span class="spec-v" title={specs.cpu_name}>{specs.cpu_name}</span>
                      <span class="spec-s mono">{specs.cpu_cores} cores</span>
                    </div>
                    <div class="local-spec-cell">
                      <span class="spec-k mono">SYSTEM MEMORY</span>
                      <span class="spec-v">{fmtGb(specs.ram_mb)} RAM</span>
                      <span class="spec-s mono">
                        {specs.unified_memory ? 'Unified memory' : 'Host memory'}
                      </span>
                    </div>
                    <div class="local-spec-cell">
                      <span class="spec-k mono">GRAPHICS / ACCEL</span>
                      {#if specs.gpu_name}
                        <span class="spec-v" title={specs.gpu_name}>{specs.gpu_name}</span>
                        <span class="spec-s mono">
                          {specs.unified_memory
                            ? `~${fmtGb(specs.effective_gpu_mb)} GPU budget`
                            : `${fmtGb(specs.vram_mb)} VRAM`}
                        </span>
                      {:else}
                        <span class="spec-v">CPU Only</span>
                        <span class="spec-s mono">No discrete GPU detected</span>
                      {/if}
                    </div>
                    <div class="local-spec-cell">
                      <span class="spec-k mono">RUNTIME ENGINE</span>
                      {#if localStatus.runtime_installed}
                        <span class="spec-v good">
                          llama.cpp {localStatus.runtime_installed}
                        </span>
                        <span class="spec-s mono">
                          {localStatus.runtime_backend ?? specs.recommended_backend} ready
                        </span>
                      {:else}
                        <span class="spec-v">Auto-installs</span>
                        <span class="spec-s mono">~35 MB on first launch</span>
                      {/if}
                    </div>
                  </div>

                  {#if localBusy === 'install' || localStatus.downloading_runtime || localRuntimeDl}
                    <div class="dl-box">
                      <div class="dl-meta mono">
                        <span>
                          Downloading llama.cpp ({localRuntimeDl?.backend || specs.recommended_backend})&hellip;
                        </span>
                        <span>
                          {#if localRuntimeDl?.totalMb}
                            {localRuntimeDl.downloadedMb} / {localRuntimeDl.totalMb} MB
                          {:else if localRuntimeDl}
                            {localRuntimeDl.downloadedMb} MB
                          {/if}
                        </span>
                      </div>
                      <div class="dl-track">
                        <div
                          class="dl-fill"
                          class:indet={!localRuntimeDl?.totalMb}
                          style={localRuntimeDl?.totalMb
                            ? `width: ${Math.min(100, Math.round((localRuntimeDl.downloadedMb / localRuntimeDl.totalMb) * 100))}%`
                            : undefined}
                        ></div>
                      </div>
                    </div>
                  {/if}

                  <div class="local-prefs-row">
                    <label class="fld">
                      <span class="fld-label">Acceleration</span>
                      <select
                        class="input mono"
                        value={localStatus.preferred_backend}
                        disabled={localBusy !== null}
                        onchange={(e) => void setLocalPrefs(e.currentTarget.value, null)}
                      >
                        <option value="auto">
                          Auto (Recommended: {specs.recommended_backend.toUpperCase()})
                        </option>
                        {#each specs.available_backends as b (b)}
                          <option value={b}>{backendLabel(b)}</option>
                        {/each}
                      </select>
                    </label>

                    <label class="fld">
                      <span class="fld-label">Context window</span>
                      <select
                        class="input mono"
                        value={String(localStatus.context_tokens)}
                        disabled={localBusy !== null}
                        onchange={(e) => void setLocalPrefs(null, Number(e.currentTarget.value))}
                      >
                        <option value="0">Auto (tuned to your PC)</option>
                        <option value="8192">8,192 tokens (low memory)</option>
                        <option value="16384">16,384 tokens (balanced)</option>
                        <option value="32768">32,768 tokens (long context)</option>
                      </select>
                    </label>
                  </div>

                  {#if !localStatus.runtime_installed && localBusy !== 'install'}
                    <div class="deck-actions">
                      <button
                        class="btn sm"
                        onclick={() => void installLocalRuntime()}
                        disabled={localBusy !== null}
                      >
                        <Icon name="download" size={11} />
                        Pre-install runtime engine
                      </button>
                      <span class="fld-hint">
                        Or click <strong>Run &amp; Use</strong> on any model &mdash; it installs automatically.
                      </span>
                    </div>
                  {/if}
                </div>

                {#if localNotice}
                  <div class="notice-banner">
                    <Icon name="info" size={13} />
                    <span>{localNotice}</span>
                    <button
                      class="notice-dismiss"
                      onclick={() => (localNotice = null)}
                      aria-label="Dismiss notice"
                    >
                      <Icon name="x" size={11} />
                    </button>
                  </div>
                {/if}

                {#if localError || error}
                  <p class="error-banner">{localError ?? error}</p>
                {/if}

                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">MODELS FOR YOUR PC</span>
                    <div class="deck-head-right">
                      <span class="deck-sub mono">
                        {localStatus.models.filter((m) => m.downloaded).length} downloaded
                      </span>
                      <button
                        class="btn sm ghost"
                        onclick={() => void refreshLocalStatus(true)}
                        disabled={localLoading}
                      >
                        <span class="icon-spin" class:spinning={localLoading}>
                          <Icon name="refresh" size={11} />
                        </span>
                        {localLoading ? 'Scanning…' : 'Refresh'}
                      </button>
                    </div>
                  </div>

                  <p class="deck-copy">
                    Downloads run with SHA-256 verification and automatic GPU-to-CPU fallback so models run reliably on any PC.
                  </p>

                  <div class="local-models-list">
                    {#each sortedLocalModels as m (m.id)}
                      {@const isDl = activeDownloadingId === m.id}
                      {@const dlPct = isDl && localDlProgress?.modelId === m.id ? localDlProgress.pct : 0}
                      {@const dlMb = isDl && localDlProgress?.modelId === m.id ? localDlProgress.downloadedMb : m.partial_mb}
                      {@const dlTotal = isDl && localDlProgress?.modelId === m.id && localDlProgress.totalMb ? localDlProgress.totalMb : m.size_mb}
                      {@const startingThis = localBusy === `start:${m.id}`}
                      {@const deletingThis = localBusy === `del:${m.id}`}
                      {@const isRunningThis = m.active && localStatus.server_running}

                      <div
                        class="local-model-card"
                        class:recommended={m.recommended}
                        class:live={isRunningThis}
                        class:heavy={m.fit === 'too_heavy' && !m.downloaded}
                      >
                        <div class="lm-top">
                          <div class="lm-title-col">
                            <div class="lm-name-row">
                              <span class="lm-name">{m.name}</span>
                              <span class="spec-chip mono">{m.params} &middot; {m.quant}</span>
                              <span class="spec-chip mono">{fmtGb(m.size_mb)}</span>
                              {#if m.custom}
                                <span class="spec-chip mono">Custom</span>
                              {/if}
                            </div>
                            <p class="lm-summary">{m.summary}</p>
                          </div>

                          <div class="lm-badges">
                            {#if isRunningThis}
                              <span class="fit-pill live mono">Running</span>
                            {:else if m.recommended}
                              <span class="fit-pill rec mono">Recommended</span>
                            {/if}
                            <span
                              class="fit-pill mono"
                              class:gpu={m.fit === 'gpu'}
                              class:hybrid={m.fit === 'hybrid'}
                              class:cpu={m.fit === 'cpu'}
                              class:heavy={m.fit === 'too_heavy'}
                            >
                              {fitBadgeLabel(m, specs)}
                            </span>
                          </div>
                        </div>

                        <div class="lm-fit-line mono">
                          <span>{m.fit_note}</span>
                          <span>&middot;</span>
                          <span>{formatCtx(m.recommended_ctx) ?? '16k ctx'}</span>
                          {#if m.tool_calling}
                            <span>&middot; Tools</span>
                          {/if}
                        </div>

                        {#if isDl || (m.partial_mb > 0 && !m.downloaded)}
                          <div class="dl-box">
                            <div class="dl-meta mono">
                              <span>{isDl ? 'Downloading model&hellip;' : 'Paused partial download'}</span>
                              <span>{fmtGb(dlMb)} / {fmtGb(dlTotal)} ({isDl ? dlPct : Math.round((m.partial_mb / Math.max(1, m.size_mb)) * 100)}%)</span>
                            </div>
                            <div class="dl-track">
                              <div
                                class="dl-fill"
                                style="width: {isDl ? dlPct : Math.min(100, Math.round((m.partial_mb / Math.max(1, m.size_mb)) * 100))}%"
                              ></div>
                            </div>
                          </div>
                        {/if}

                        <div class="lm-actions">
                          {#if isDl}
                            <button
                              class="btn sm ghost"
                              onclick={() => void cancelLocalDownload()}
                            >
                              Cancel download
                            </button>
                          {:else if m.downloaded}
                            {#if isRunningThis && item.isDefault && session.providerName === 'local' && session.model === m.id}
                              <span class="active-badge mono">
                                <Icon name="check" size={11} />
                                Active in chat
                              </span>
                            {:else}
                              <button
                                class="btn primary sm"
                                onclick={() => void startLocalModel(m.id)}
                                disabled={localBusy !== null}
                              >
                                {startingThis
                                  ? 'Starting…'
                                  : isRunningThis
                                    ? 'Use in chat'
                                    : 'Run & Use'}
                              </button>
                            {/if}
                            <button
                              class="remove-btn"
                              class:confirming={confirmDeleteModel === m.id}
                              onclick={() => void deleteLocalModel(m.id, m.name)}
                              disabled={deletingThis || startingThis}
                              title="Uninstall model from disk"
                            >
                              {deletingThis
                                ? 'Uninstalling…'
                                : confirmDeleteModel === m.id
                                  ? 'Click again to uninstall'
                                  : 'Uninstall'}
                            </button>
                            {#if confirmDeleteModel === m.id && !deletingThis}
                              <button
                                class="btn sm ghost"
                                onclick={() => {
                                  confirmDeleteModel = null;
                                  if (confirmDeleteTimer) clearTimeout(confirmDeleteTimer);
                                }}
                              >
                                Cancel
                              </button>
                            {/if}
                          {:else}
                            <button
                              class={m.recommended ? 'btn primary sm' : 'btn sm'}
                              onclick={() => void downloadLocalModel(m.id)}
                              disabled={localBusy !== null || !!activeDownloadingId}
                            >
                              <Icon name="download" size={11} />
                              {m.partial_mb > 0
                                ? `Resume (${fmtGb(m.partial_mb)})`
                                : `Download (${fmtGb(m.size_mb)})`}
                            </button>
                            {#if m.partial_mb > 0}
                              <button
                                class="remove-btn"
                                onclick={() => void deleteLocalModel(m.id, m.name, true)}
                                disabled={deletingThis}
                                title="Delete incomplete partial download"
                              >
                                {deletingThis ? 'Discarding…' : 'Discard'}
                              </button>
                            {/if}
                          {/if}
                        </div>
                      </div>
                    {/each}
                  </div>

                  <div class="local-dir-foot">
                    <span class="mono local-dir-label">MODELS FOLDER</span>
                    <code class="mono local-dir-path" title={localStatus.models_dir}>
                      {localStatus.models_dir}
                    </code>
                    <button
                      class="copy-mini"
                      onclick={() => void copyModelsDir(localStatus!.models_dir)}
                      title="Copy models folder path"
                      aria-label="Copy models folder path"
                    >
                      <Icon name={localModelsCopied ? 'check' : 'copy'} size={11} />
                    </button>
                  </div>
                </div>

                {#if p}
                  <div class="stage-danger-foot">
                    <span class="foot-note">
                      Stops the local server and removes Built-in Local from your active provider list (downloaded GGUF files stay on disk).
                    </span>
                    <div class="danger-actions">
                      <button
                        class="remove-btn"
                        class:confirming={confirmRemove === p.name}
                        onclick={() => void removeProvider(p)}
                        disabled={localBusy === 'stop'}
                      >
                        {confirmRemove === p.name ? 'Click again to disable' : 'Disable local provider'}
                      </button>
                      {#if confirmRemove === p.name}
                        <button
                          class="btn sm ghost"
                          onclick={() => {
                            confirmRemove = null;
                            if (confirmTimer) clearTimeout(confirmTimer);
                          }}
                        >
                          Cancel
                        </button>
                      {/if}
                    </div>
                  </div>
                {/if}
              {/if}
            {:else if p}
              <!-- INSTALLED PROVIDER CONTROLS -->
              {#if isAcp}
                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">GOOGLE ANTIGRAVITY AGENT</span>
                    {#if agStatus?.installed}
                      <span class="deck-sub mono">v{agStatus.installed}</span>
                    {/if}
                  </div>

                  {#if agStatus && !agStatus.supported}
                    <p class="deck-copy">Google does not publish Antigravity for this platform.</p>
                  {:else if !agStatus?.installed}
                    <p class="deck-copy">
                      Downloads Google's official Antigravity agent (~100 MB, checksum-verified) so you can run models using your Free, Pro, or Ultra plan.
                    </p>
                    <div class="deck-actions">
                      <button
                        class="btn primary"
                        onclick={() => agRun('install', (m) => m.antigravityInstall())}
                        disabled={agBusy !== null}
                      >
                        {agBusy === 'install' ? agProgressLabel() : 'Install Antigravity'}
                      </button>
                    </div>
                  {:else if agBusy === 'login'}
                    <div class="oauth-box">
                      <p class="oauth-copy">Finish signing in with Google in your browser…</p>
                      <div class="deck-actions">
                        {#if agLoginUrl}
                          <button class="btn" onclick={agCopyLink}>
                            {agCopied ? 'Copied link' : 'Copy sign-in link'}
                          </button>
                        {/if}
                        <button class="btn ghost" onclick={agCancelLogin}>Cancel</button>
                      </div>
                    </div>
                  {:else if !agStatus.signed_in}
                    <p class="deck-copy">
                      Antigravity v{agStatus.installed} is installed. Sign in with Google to unlock models.
                    </p>
                    <div class="deck-actions">
                      <button
                        class="btn primary"
                        onclick={() => agRun('login', (m) => m.antigravityLogin())}
                        disabled={agBusy !== null}
                      >
                        Sign in with Google
                      </button>
                      <span class="fld-hint">Free / Pro / Ultra plan</span>
                    </div>
                  {:else}
                    <div class="ag-live">
                      <div class="ag-stat">
                        <span class="beacon sm ok"></span>
                        <span>Signed in with Google · {agStatus.models} model{agStatus.models === 1 ? '' : 's'} ready</span>
                      </div>
                      <div class="deck-actions">
                        <button
                          class="btn sm"
                          onclick={() => agRun('refresh', (m) => m.antigravityRefresh())}
                          disabled={agBusy !== null}
                        >
                          <Icon name="refresh" size={11} />
                          {agBusy === 'refresh' ? 'Refreshing…' : 'Refresh models'}
                        </button>
                        <button
                          class="btn sm ghost"
                          onclick={() => agRun('logout', (m) => m.antigravityLogout())}
                          disabled={agBusy !== null}
                        >
                          Sign out
                        </button>
                      </div>
                    </div>
                  {/if}

                  {#if agError}
                    <p class="error-banner">{agError}</p>
                  {/if}
                </div>
              {:else}
                <!-- MODEL SELECTION DECK -->
                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">DEFAULT MODEL</span>
                    <div class="deck-head-right">
                      {#if modelLists[p.name]?.length}
                        <span class="deck-sub mono">{modelLists[p.name]?.length} models</span>
                      {/if}
                      <button
                        class="btn sm ghost"
                        onclick={() => void loadProviderModels(p.name)}
                        disabled={modelBusy === p.name}
                      >
                        <Icon name="refresh" size={11} />
                        {modelBusy === p.name ? 'Loading…' : modelLists[p.name]?.length ? 'Refresh' : 'Load models'}
                      </button>
                    </div>
                  </div>

                  {#if (modelLists[p.name]?.length ?? 0) > 12}
                    <input
                      class="input mono model-filter-input"
                      type="search"
                      placeholder="Filter {modelLists[p.name]?.length} models…"
                      bind:value={modelFilter}
                    />
                  {/if}

                  <select
                    class="input mono model-picker"
                    aria-label="Model for {item.label}"
                    value={p.model}
                    onchange={(e) => {
                      const val = e.currentTarget.value;
                      if (val === '__load') void loadProviderModels(p.name);
                      else if (val) void pickModel(p.name, val);
                    }}
                  >
                    {#if !filteredLoadedModels.some((x) => x.id === p.model)}
                      <option value={p.model}>{p.model || 'no model selected'}</option>
                    {/if}
                    {#if !modelLists[p.name]?.length}
                      <option value="__load">Load live model list…</option>
                    {/if}
                    {#each filteredLoadedModels as m (m.id)}
                      <option value={m.id}>
                        {m.id}{m.name && m.name !== m.id ? ` — ${m.name}` : ''}
                      </option>
                    {/each}
                  </select>

                  {#if activeModelMeta}
                    <div class="model-specs mono">
                      {#if formatCtx(activeModelMeta.context_tokens)}
                        <span class="spec-chip">{formatCtx(activeModelMeta.context_tokens)}</span>
                      {/if}
                      {#if activeModelMeta.price_in_mtok != null && activeModelMeta.price_out_mtok != null}
                        <span class="spec-chip">
                          ${activeModelMeta.price_in_mtok} / ${activeModelMeta.price_out_mtok} MTok
                        </span>
                      {/if}
                      {#if activeModelMeta.supports_vision}
                        <span class="spec-chip">vision</span>
                      {/if}
                      {#if activeModelMeta.supports_tools}
                        <span class="spec-chip">tools</span>
                      {/if}
                    </div>
                  {/if}
                </div>

                <!-- CREDENTIALS & AUTH DECK -->
                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">AUTHENTICATION</span>
                    <span class="deck-sub mono">
                      {#if p.auth_kind === 'oauth'}
                        OAuth active
                      {:else if p.auth_kind === 'api_key'}
                        Keychain stored
                      {:else if p.auth_kind === 'env'}
                        From {item.envHint ?? 'environment'}
                      {:else if preset?.auth === 'none'}
                        No key required
                      {:else}
                        Unauthenticated
                      {/if}
                    </span>
                  </div>

                  {#if p.name === 'nous'}
                    {#if login}
                      <div class="oauth-box">
                        <p class="oauth-copy">
                          Approve sign-in at
                          <a href={login.verification_url} target="_blank" rel="noreferrer">
                            {login.verification_url}
                          </a>
                          using code <code class="mono">{login.user_code}</code>
                        </p>
                        <div class="deck-actions">
                          <button class="btn ghost sm" onclick={cancelLogin}>Cancel</button>
                        </div>
                      </div>
                    {:else}
                      <div class="deck-actions">
                        <button
                          class="btn primary"
                          onclick={connectNous}
                          disabled={loginBusy || (p.auth_kind !== 'none' && p.auth_kind !== 'env')}
                        >
                          {p.auth_kind === 'oauth' ? 'Connected via Nous Portal' : 'Connect Nous Portal'}
                        </button>
                      </div>
                    {/if}
                    {#if loginError}
                      <p class="error-banner">{loginError}</p>
                    {/if}
                  {/if}

                  {#if p.name === 'claude'}
                    {#if claudeLogin}
                      <div class="oauth-box">
                        <p class="oauth-copy">
                          Authorize in browser at
                          <a href={claudeLogin.authorize_url} target="_blank" rel="noreferrer">
                            {claudeLogin.authorize_url}
                          </a>
                        </p>
                        <div class="key-input-row">
                          <input
                            class="input mono"
                            type="text"
                            placeholder="Paste the authorization code…"
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
                      <div class="deck-actions">
                        <button class="btn primary" onclick={connectClaude} disabled={claudeBusy}>
                          {p.auth_kind === 'oauth' ? 'Reconnect Claude Subscription' : 'Sign in with Claude'}
                        </button>
                      </div>
                    {/if}
                    {#if claudeError}
                      <p class="error-banner">{claudeError}</p>
                    {/if}
                  {/if}

                  {#if (p.auth_kind === 'none' || p.auth_kind === 'env' || p.auth_kind === 'api_key') && !(p.name === 'nous' && login) && !(p.name === 'claude' && claudeLogin)}
                    <div class="key-input-row" class:spaced={p.name === 'nous' || p.name === 'claude'}>
                      <div class="secret-wrap">
                        <input
                          class="input mono"
                          type={showInstalledKey[p.name] ? 'text' : 'password'}
                          placeholder={p.auth_kind === 'api_key'
                            ? 'Replace stored API key…'
                            : preset?.auth === 'none'
                              ? 'Optional bearer token…'
                              : `Paste ${item.envHint ?? 'API key'}…`}
                          bind:value={keyDrafts[p.name]}
                          onkeydown={(e) => e.key === 'Enter' && saveKey(p.name)}
                        />
                        <button
                          type="button"
                          class="eye-btn"
                          onclick={() => (showInstalledKey[p.name] = !showInstalledKey[p.name])}
                          aria-label={showInstalledKey[p.name] ? 'Hide key' : 'Show key'}
                        >
                          <Icon name={showInstalledKey[p.name] ? 'eye-off' : 'eye'} size={13} />
                        </button>
                      </div>
                      <button
                        class="btn"
                        onclick={() => void saveKey(p.name)}
                        disabled={!keyDrafts[p.name]?.trim() || keyBusy === p.name}
                      >
                        {keyBusy === p.name ? 'Saving…' : p.auth_kind === 'api_key' ? 'Update key' : 'Save key'}
                      </button>
                    </div>
                  {/if}

                  {#if p.auth_kind === 'api_key' || p.auth_kind === 'oauth'}
                    <div class="cred-foot">
                      <button
                        class="text-link mono"
                        onclick={() => void disconnectCredentials(p)}
                        disabled={logoutBusy === p.name}
                      >
                        {logoutBusy === p.name ? 'Clearing…' : 'Clear stored credentials'}
                      </button>
                    </div>
                  {/if}
                </div>
              {/if}

              {#if error}
                <p class="error-banner">{error}</p>
              {/if}

              <div class="stage-danger-foot">
                <span class="foot-note">
                  Removing forgets this provider’s saved key or sign-in.
                </span>
                <button
                  class="remove-btn"
                  class:confirming={confirmRemove === p.name}
                  onclick={() => void removeProvider(p)}
                >
                  {confirmRemove === p.name ? 'Click again to remove' : 'Remove provider'}
                </button>
              </div>
            {:else if preset}
              <!-- UNINSTALLED CATALOG PRESET: INLINE SETUP ON THE STAGE -->
              {#if preset.auth === 'agent'}
                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">SETUP</span>
                  </div>
                  <p class="deck-copy">
                    Enable Google’s official Antigravity agent to download it locally and sign in with your Google Free, Pro, or Ultra plan.
                  </p>
                  <label class="check-row">
                    <input type="checkbox" bind:checked={setupUseNow} />
                    <span>Make this the active provider once enabled</span>
                  </label>
                  <div class="deck-actions">
                    <button
                      class="btn primary"
                      onclick={() => void finishSetup('default')}
                      disabled={setupBusy}
                    >
                      {setupBusy ? 'Enabling…' : 'Enable Antigravity'}
                    </button>
                  </div>
                </div>
              {:else}
                {#if preset.auth === 'oauth'}
                  <div class="deck-card oauth-hero">
                    <div class="deck-head">
                      <span class="deck-title mono">RECOMMENDED · SUBSCRIPTION SIGN-IN</span>
                    </div>
                    <p class="deck-copy">
                      Sign in with your {preset.label} account in one step, or configure an API key below instead.
                    </p>
                    <div class="deck-actions">
                      <button
                        class="btn primary"
                        onclick={() => void finishSetup('oauth')}
                        disabled={setupBusy}
                      >
                        {setupBusy ? 'Starting…' : `Add & Sign in with ${preset.label}`}
                      </button>
                    </div>
                  </div>
                {/if}

                <div class="deck-card">
                  <div class="deck-head">
                    <span class="deck-title mono">
                      {preset.auth === 'none'
                        ? 'LOCAL SERVER CONNECTION'
                        : preset.auth === 'oauth'
                          ? 'OR CONNECT WITH API KEY'
                          : 'API KEY & MODEL'}
                    </span>
                  </div>

                  {#if preset.auth !== 'none'}
                    <div class="fld">
                      <span class="fld-label">API key</span>
                      <div class="key-input-row">
                        <div class="secret-wrap">
                          <input
                            class="input mono"
                            type={setupShowKey ? 'text' : 'password'}
                            placeholder={preset.env_hint ? `${preset.env_hint} or paste key…` : 'Paste your API key…'}
                            bind:value={setupKey}
                            onkeydown={(e) => e.key === 'Enter' && canFinishSetup && finishSetup('default')}
                          />
                          <button
                            type="button"
                            class="eye-btn"
                            onclick={() => (setupShowKey = !setupShowKey)}
                            aria-label={setupShowKey ? 'Hide key' : 'Show key'}
                          >
                            <Icon name={setupShowKey ? 'eye-off' : 'eye'} size={13} />
                          </button>
                        </div>
                        <button
                          class="btn"
                          onclick={scanSetupModels}
                          disabled={setupScanning || !setupBaseUrl.trim() || !setupKey.trim()}
                          title="Verify this key and fetch available models"
                        >
                          {setupScanning ? 'Checking…' : 'Check key'}
                        </button>
                      </div>
                      <span class="fld-hint">
                        {#if setupScanError}
                          <span class="bad">{setupScanError}</span>
                        {:else if setupModels.length > 0}
                          <span class="good">Key verified — {setupModels.length} models ready.</span>
                        {:else if preset.env_hint}
                          Leave empty to read <code class="mono">{preset.env_hint}</code> from your environment, or add a key later.
                        {:else}
                          Stored locally in your OS keychain.
                        {/if}
                      </span>
                    </div>
                  {:else}
                    <div class="fld">
                      <span class="fld-label">Server URL</span>
                      <div class="key-input-row">
                        <input
                          class="input mono"
                          type="text"
                          bind:value={setupBaseUrl}
                          spellcheck="false"
                        />
                        <button
                          class="btn"
                          onclick={scanSetupModels}
                          disabled={setupScanning || !setupBaseUrl.trim()}
                          title="Check if the local server is running and list its models"
                        >
                          {setupScanning ? 'Probing…' : 'Probe server'}
                        </button>
                      </div>
                      {#if setupScanError}
                        <span class="fld-hint bad">{setupScanError}</span>
                      {:else if setupModels.length > 0}
                        <span class="fld-hint good">Server online — {setupModels.length} local models found.</span>
                      {:else}
                        <span class="fld-hint">Make sure the local runtime is running before sending a message.</span>
                      {/if}
                    </div>
                  {/if}

                  <label class="fld">
                    <span class="fld-label">
                      Starting model
                      {#if setupModels.length > 0}
                        <span class="fld-badge mono">{setupModels.length} available</span>
                      {/if}
                    </span>
                    {#if setupModels.length > 0}
                      <select class="input mono" bind:value={setupModel}>
                        {#each setupModels as m (m.id)}
                          <option value={m.id}>{m.id}{m.name && m.name !== m.id ? ` · ${m.name}` : ''}</option>
                        {/each}
                      </select>
                    {:else}
                      <input class="input mono" type="text" bind:value={setupModel} spellcheck="false" />
                    {/if}
                  </label>

                  {#if preset.auth !== 'none'}
                    <button
                      class="advanced-toggle"
                      onclick={() => (setupShowAdvanced = !setupShowAdvanced)}
                    >
                      <Icon name={setupShowAdvanced ? 'chevron-down' : 'chevron-right'} size={10} />
                      Custom proxy or regional URL
                    </button>
                    {#if setupShowAdvanced}
                      <label class="fld">
                        <span class="fld-label">Base URL</span>
                        <input class="input mono" type="text" bind:value={setupBaseUrl} spellcheck="false" />
                      </label>
                    {/if}
                  {/if}

                  <label class="check-row">
                    <input type="checkbox" bind:checked={setupUseNow} />
                    <span>Switch to {preset.label} as the active provider</span>
                  </label>

                  {#if setupError}
                    <p class="error-banner">{setupError}</p>
                  {/if}

                  <div class="stage-actions-foot">
                    <button
                      class="btn primary"
                      onclick={() => void finishSetup('default')}
                      disabled={!canFinishSetup || setupBusy}
                    >
                      {setupBusy
                        ? 'Connecting…'
                        : preset.auth === 'none'
                          ? `Connect ${preset.label}`
                          : `Save & Add ${preset.label}`}
                    </button>
                  </div>
                </div>
              {/if}
            {/if}
          </div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .prov-wrap {
    container-type: inline-size;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .prov-top {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .prov-heading .group-title {
    font-family: var(--font-ui);
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--fg-0);
    margin: 0 0 4px;
  }
  .prov-heading .group-hint {
    margin: 0;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--fg-2);
    line-height: 1.5;
  }

  .route-telemetry {
    display: flex;
    align-items: center;
  }
  .active-route {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    max-width: 100%;
    padding: 5px 11px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border-strong);
    background: var(--bg-1);
    box-shadow: var(--el-1);
    font-size: 11.5px;
    color: var(--fg-1);
    transition: border-color var(--motion), background var(--motion);
  }
  .active-route:not(.empty):hover {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border-strong));
    background: var(--bg-2);
  }
  .active-route.empty {
    cursor: default;
    color: var(--fg-3);
  }
  .route-label {
    font-size: 9.5px;
    letter-spacing: 0.12em;
    color: var(--fg-3);
  }
  .route-mark,
  .chip-mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-1);
    flex: none;
  }
  .route-chip.selected .chip-mark {
    color: var(--accent);
  }
  .route-name {
    font-weight: 600;
    color: var(--fg-0);
  }
  .route-sep {
    color: var(--fg-3);
  }
  .route-model {
    font-size: 11px;
    color: var(--accent);
    max-width: 210px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Quick-jump chips for configured providers */
  .route-strip {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 8px 12px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: 12px;
  }
  .strip-caption {
    font-size: 9.5px;
    letter-spacing: 0.14em;
    color: var(--fg-3);
    flex: none;
  }
  .strip-chips {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .route-chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 27px;
    padding: 0 10px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-0);
    color: var(--fg-2);
    font-size: 12px;
    transition: border-color var(--motion), color var(--motion), background var(--motion);
  }
  .route-chip:hover {
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .route-chip.selected {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 48%, transparent);
    color: var(--fg-0);
  }
  .chip-name {
    font-weight: 500;
  }
  .chip-tag {
    font-size: 9px;
    letter-spacing: 0.08em;
    color: var(--accent);
  }

  /* Status beacons */
  .beacon {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--fg-3);
    flex: none;
  }
  .beacon.sm {
    width: 6px;
    height: 6px;
  }
  .beacon.ok {
    background: var(--ok);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 18%, transparent);
  }
  .beacon.warn {
    background: transparent;
    border: 1.5px solid var(--warn);
  }

  .offline-card {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 18px;
    border-radius: var(--radius-lg);
    border: 1px solid var(--border);
    background: var(--bg-1);
    color: var(--fg-2);
    font-family: var(--font-mono);
    font-size: 12px;
  }

  /* Unified Two-Pane Switchboard */
  .switchboard {
    display: grid;
    grid-template-columns: 278px minmax(0, 1fr);
    min-height: 520px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--el-1);
    overflow: hidden;
  }

  /* Left Rail: Unified Atlas */
  .atlas {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    max-height: 610px;
    background: color-mix(in srgb, var(--bg-0) 45%, var(--bg-1));
    border-right: 1px solid var(--border);
  }
  .atlas-search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-3);
  }
  .atlas-input {
    flex: 1 1 auto;
    min-width: 0;
    background: transparent;
    border: none;
    outline: none;
    font-size: 12px;
    color: var(--fg-0);
  }
  .atlas-input::placeholder {
    color: var(--fg-3);
  }
  .atlas-clear {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: 5px;
    color: var(--fg-3);
  }
  .atlas-clear:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }

  .atlas-cats {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border);
  }
  .cat-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    border-radius: var(--radius-pill);
    font-size: 11px;
    color: var(--fg-2);
    background: transparent;
    border: 1px solid transparent;
    transition: background var(--motion), color var(--motion), border-color var(--motion);
  }
  .cat-pill:hover {
    color: var(--fg-0);
    background: var(--bg-2);
  }
  .cat-pill.on {
    color: var(--accent);
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 32%, transparent);
  }
  .cat-n {
    font-size: 9.5px;
    opacity: 0.75;
  }

  .atlas-list {
    flex: 1 1 auto;
    overflow-y: auto;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .atlas-group-label {
    font-size: 9.5px;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--fg-3);
    padding: 10px 8px 4px;
  }
  .atlas-group-label:first-child {
    padding-top: 4px;
  }
  .atlas-row {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 7px 8px;
    border-radius: 9px;
    border: 1px solid transparent;
    text-align: left;
    transition: background var(--motion), border-color var(--motion);
  }
  .atlas-row:hover {
    background: var(--bg-2);
  }
  .atlas-row.selected {
    background: color-mix(in srgb, var(--accent) 10%, var(--bg-2));
    border-color: color-mix(in srgb, var(--accent) 35%, transparent);
  }
  .row-mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    flex: none;
    border-radius: 7px;
    font-family: 'm5x7', var(--font-mono);
    font-size: 16px;
    color: var(--fg-2);
    background: var(--bg-2);
    border: 1px solid var(--border);
  }
  .atlas-row.selected .row-mark,
  .row-mark.live {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-2));
    border-color: color-mix(in srgb, var(--accent) 30%, var(--border));
  }
  .row-copy {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1 1 auto;
    gap: 1px;
  }
  .row-name {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--fg-0);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-sub {
    font-size: 10px;
    color: var(--fg-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-status {
    display: inline-flex;
    align-items: center;
    flex: none;
  }
  .row-tag {
    font-size: 9px;
    letter-spacing: 0.08em;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .row-arrow {
    color: var(--fg-3);
    opacity: 0;
    transition: opacity var(--motion);
  }
  .atlas-row:hover .row-arrow,
  .atlas-row.selected .row-arrow {
    opacity: 1;
  }

  .atlas-empty {
    padding: 28px 14px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: var(--fg-3);
    font-size: 12px;
  }
  .atlas-empty p {
    margin: 0;
  }

  .atlas-foot {
    padding: 8px;
    border-top: 1px solid var(--border);
    background: var(--bg-1);
  }
  .custom-trigger {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 7px 9px;
    border-radius: 9px;
    border: 1px dashed var(--border-strong);
    text-align: left;
    transition: border-color var(--motion), background var(--motion);
  }
  .custom-trigger:hover,
  .custom-trigger.selected {
    border-style: solid;
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-soft);
  }
  .custom-plus {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    background: var(--bg-2);
    color: var(--accent);
    flex: none;
  }
  .custom-copy {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .custom-title {
    font-size: 12px;
    font-weight: 500;
    color: var(--fg-0);
  }
  .custom-sub {
    font-size: 10px;
    color: var(--fg-3);
  }

  /* Right Pane: Dedicated Configuration Stage */
  .stage {
    display: flex;
    flex-direction: column;
    min-width: 0;
    background:
      radial-gradient(
        100% 240px at 90% 0%,
        color-mix(in srgb, var(--accent) 7%, transparent),
        transparent 70%
      ),
      var(--bg-1);
  }
  .stage-head {
    position: relative;
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    padding: 20px 22px 16px;
    border-bottom: 1px solid var(--border);
  }
  .stage-head::before {
    content: '';
    position: absolute;
    inset: 0;
    pointer-events: none;
    background-image: radial-gradient(
      color-mix(in srgb, var(--fg-0) 10%, transparent) 1px,
      transparent 1px
    );
    background-size: 16px 16px;
    opacity: 0.45;
    mask-image: linear-gradient(135deg, transparent 25%, #000 100%);
    -webkit-mask-image: linear-gradient(135deg, transparent 25%, #000 100%);
  }
  .stage-head.fresh {
    animation: stage-flash 2.2s ease-out;
  }
  @keyframes stage-flash {
    0%,
    35% {
      background: color-mix(in srgb, var(--ok) 14%, transparent);
    }
    100% {
      background: transparent;
    }
  }

  .stage-emblem {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    flex: none;
    border-radius: 13px;
    font-family: 'm5x7', var(--font-mono);
    font-size: 28px;
    color: var(--fg-1);
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    box-shadow: var(--el-1);
  }
  .stage-emblem.live,
  .stage-emblem.custom {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-2));
    border-color: color-mix(in srgb, var(--accent) 36%, var(--border-strong));
  }
  .stage-emblem.armed {
    border-color: color-mix(in srgb, var(--warn) 42%, var(--border-strong));
  }
  .emblem-dot {
    position: absolute;
    right: -3px;
    bottom: -3px;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--bg-3);
    border: 2px solid var(--bg-1);
  }
  .emblem-dot.ok {
    background: var(--ok);
  }
  .emblem-dot.warn {
    background: var(--warn);
  }

  .stage-title-wrap {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
    flex: 1 1 200px;
  }
  .stage-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .stage-title {
    margin: 0;
    font-size: 19px;
    font-weight: 600;
    letter-spacing: -0.015em;
    color: var(--fg-0);
  }
  .slug-tag {
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .state-pill {
    font-size: 9.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 2px 8px;
    border-radius: var(--radius-pill);
    background: var(--bg-3);
    color: var(--fg-2);
  }
  .state-pill.ok {
    background: color-mix(in srgb, var(--ok) 15%, var(--bg-3));
    color: var(--ok);
  }
  .state-pill.warn {
    background: color-mix(in srgb, var(--warn) 16%, var(--bg-3));
    color: var(--warn);
  }

  .stage-meta-row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .meta-chip {
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: 6px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    color: var(--fg-2);
  }
  .key-link {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 10.5px;
    padding: 2px 8px;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, var(--accent) 28%, var(--border));
    color: var(--accent);
    text-decoration: none;
    transition: background var(--motion);
  }
  .key-link:hover {
    background: var(--accent-soft);
  }

  .stage-head-cta {
    position: relative;
    margin-left: auto;
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .danger-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .icon-spin {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .icon-spin.spinning {
    animation: spin-rot 0.8s linear infinite;
  }
  @keyframes spin-rot {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }
  .active-badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 10.5px;
    padding: 5px 10px;
    border-radius: var(--radius-pill);
    background: var(--accent-soft);
    border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
    color: var(--accent);
  }

  .endpoint-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 22px;
    background: color-mix(in srgb, var(--bg-0) 60%, var(--bg-1));
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .endpoint-label {
    font-size: 9.5px;
    letter-spacing: 0.12em;
    color: var(--fg-3);
    flex: none;
  }
  .endpoint-url {
    font-size: 11px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1 1 auto;
    min-width: 0;
  }
  .env-chip {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 5px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    color: var(--fg-2);
    flex: none;
  }
  .copy-mini {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    color: var(--fg-3);
    flex: none;
  }
  .copy-mini:hover {
    color: var(--fg-0);
    background: var(--bg-2);
  }

  .stage-body {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px 22px 20px;
  }
  .stage-intro {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.55;
    color: var(--fg-2);
  }

  .deck-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
    border-radius: 12px;
    background: color-mix(in srgb, var(--bg-0) 55%, var(--bg-1));
    border: 1px solid var(--border);
  }
  .deck-card.oauth-hero {
    border-color: color-mix(in srgb, var(--accent) 28%, var(--border));
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--accent) 5%, var(--bg-0)),
      color-mix(in srgb, var(--bg-0) 60%, var(--bg-1))
    );
  }
  .deck-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }
  .deck-head-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .deck-title {
    font-size: 10px;
    letter-spacing: 0.14em;
    color: var(--fg-3);
  }
  .deck-sub {
    font-size: 10.5px;
    color: var(--fg-2);
  }
  .deck-copy {
    margin: 0;
    font-size: 12px;
    line-height: 1.55;
    color: var(--fg-2);
  }
  .deck-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .model-filter-input {
    font-size: 11.5px;
    padding: 5px 9px;
  }
  .model-picker {
    font-size: 12px;
  }
  .model-specs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .spec-chip {
    font-size: 10px;
    padding: 2px 7px;
    border-radius: 5px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    color: var(--fg-2);
  }

  .form-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: 5px;
    min-width: 0;
  }
  .fld-full {
    grid-column: 1 / -1;
  }
  .fld-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11.5px;
    font-weight: 500;
    color: var(--fg-1);
  }
  .fld-badge {
    font-size: 10px;
    color: var(--fg-3);
  }
  .fld-hint {
    font-size: 10.5px;
    line-height: 1.45;
    color: var(--fg-3);
  }
  .good {
    color: var(--ok);
  }
  .bad {
    color: var(--err);
  }

  .seg-group {
    display: inline-flex;
    gap: 3px;
    padding: 3px;
    border-radius: 9px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    align-self: flex-start;
  }
  .seg-btn {
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 11px;
    color: var(--fg-2);
    transition: background var(--motion), color var(--motion);
  }
  .seg-btn:hover {
    color: var(--fg-0);
  }
  .seg-btn.on {
    background: var(--bg-0);
    color: var(--fg-0);
    box-shadow: var(--el-1);
  }

  .key-input-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .key-input-row.spaced {
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .secret-wrap {
    position: relative;
    flex: 1 1 200px;
    min-width: 0;
  }
  .secret-wrap .input {
    padding-right: 32px;
  }
  .eye-btn {
    position: absolute;
    right: 6px;
    top: 50%;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    color: var(--fg-3);
  }
  .eye-btn:hover {
    color: var(--fg-0);
    background: var(--bg-2);
  }

  .oauth-box {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 11px 13px;
    border-radius: 9px;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
  }
  .oauth-copy {
    margin: 0;
    font-size: 12px;
    line-height: 1.55;
    color: var(--fg-1);
    overflow-wrap: anywhere;
  }
  .oauth-copy a {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .oauth-copy code {
    padding: 1px 6px;
    border-radius: 5px;
    background: var(--bg-3);
    color: var(--fg-0);
  }

  .ag-live {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .ag-stat {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--fg-1);
  }

  .cred-foot {
    display: flex;
    justify-content: flex-end;
  }
  .text-link {
    font-size: 10.5px;
    color: var(--fg-3);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .text-link:hover:not(:disabled) {
    color: var(--fg-1);
  }

  .advanced-toggle {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    align-self: flex-start;
    font-size: 11px;
    color: var(--fg-3);
  }
  .advanced-toggle:hover {
    color: var(--fg-1);
  }

  .check-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--fg-2);
    cursor: pointer;
    user-select: none;
  }
  .check-row input {
    accent-color: var(--accent);
  }

  .error-banner {
    margin: 0;
    padding: 9px 12px;
    border-radius: 9px;
    border: 1px solid color-mix(in srgb, var(--err) 35%, transparent);
    background: color-mix(in srgb, var(--err) 10%, transparent);
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--err);
  }

  .stage-actions-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    padding-top: 4px;
  }
  .stage-danger-foot {
    margin-top: auto;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .foot-note {
    font-size: 11px;
    color: var(--fg-3);
  }

  .remove-btn {
    flex: none;
    font-size: 11.5px;
    padding: 5px 12px;
    border-radius: 8px;
    border: 1px solid var(--border);
    color: var(--fg-2);
    transition: color var(--motion), background var(--motion), border-color var(--motion);
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

  .stage-head-cta {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .local-spec-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }
  .local-spec-cell {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 9px 11px;
    border-radius: 9px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    min-width: 0;
  }
  .spec-k {
    font-size: 9.5px;
    letter-spacing: 0.12em;
    color: var(--fg-3);
  }
  .spec-v {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--fg-0);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .spec-v.good {
    color: var(--ok);
  }
  .spec-s {
    font-size: 10.5px;
    color: var(--fg-2);
  }

  .local-prefs-row {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
  }

  .notice-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 9px 12px;
    border-radius: 9px;
    border: 1px solid color-mix(in srgb, var(--warn) 38%, transparent);
    background: color-mix(in srgb, var(--warn) 10%, transparent);
    font-size: 12px;
    color: var(--fg-1);
  }
  .notice-banner span {
    flex: 1 1 auto;
  }
  .notice-dismiss {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    color: var(--fg-2);
    flex: none;
  }
  .notice-dismiss:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }

  .local-models-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .local-model-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 13px;
    border-radius: 10px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    transition: border-color var(--motion), background var(--motion);
  }
  .local-model-card.recommended {
    border-color: color-mix(in srgb, var(--accent) 42%, var(--border));
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--accent) 5%, var(--bg-1)),
      var(--bg-1)
    );
  }
  .local-model-card.live {
    border-color: color-mix(in srgb, var(--ok) 48%, var(--border));
  }
  .local-model-card.heavy {
    opacity: 0.78;
  }
  .lm-top {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }
  .lm-title-col {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 1 1 220px;
  }
  .lm-name-row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .lm-name {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--fg-0);
  }
  .lm-summary {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--fg-2);
  }
  .lm-badges {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
    flex: none;
  }
  .fit-pill {
    font-size: 9.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 2px 7px;
    border-radius: var(--radius-pill);
    background: var(--bg-2);
    border: 1px solid var(--border);
    color: var(--fg-2);
  }
  .fit-pill.rec {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 38%, transparent);
    color: var(--accent);
  }
  .fit-pill.live,
  .fit-pill.gpu {
    background: color-mix(in srgb, var(--ok) 14%, var(--bg-2));
    border-color: color-mix(in srgb, var(--ok) 35%, transparent);
    color: var(--ok);
  }
  .fit-pill.hybrid,
  .fit-pill.cpu {
    background: color-mix(in srgb, var(--warn) 14%, var(--bg-2));
    border-color: color-mix(in srgb, var(--warn) 35%, transparent);
    color: var(--warn);
  }
  .fit-pill.heavy {
    color: var(--fg-3);
  }
  .lm-fit-line {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 10.5px;
    color: var(--fg-2);
  }

  .dl-box {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--bg-0);
    border: 1px solid var(--border);
  }
  .dl-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 10.5px;
    color: var(--fg-1);
  }
  .dl-track {
    height: 6px;
    border-radius: var(--radius-pill);
    background: var(--bg-3);
    overflow: hidden;
  }
  .dl-fill {
    height: 100%;
    border-radius: var(--radius-pill);
    background: var(--accent);
    transition: width 120ms linear;
  }
  .dl-fill.indet {
    width: 45%;
    animation: dl-slide 1.2s ease-in-out infinite;
  }
  @keyframes dl-slide {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(240%);
    }
  }

  .lm-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding-top: 2px;
  }

  .local-dir-foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 10px;
    border-radius: 8px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    min-width: 0;
  }
  .local-dir-label {
    font-size: 9px;
    letter-spacing: 0.12em;
    color: var(--fg-3);
    flex: none;
  }
  .local-dir-path {
    font-size: 10.5px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1 1 auto;
    min-width: 0;
  }

  .muted {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--fg-3);
  }

  @container (max-width: 780px) {
    .switchboard {
      grid-template-columns: 244px minmax(0, 1fr);
    }
  }

  @container (max-width: 660px) {
    .switchboard {
      grid-template-columns: 1fr;
    }
    .atlas {
      max-height: 260px;
      border-right: none;
      border-bottom: 1px solid var(--border);
    }
    .form-grid,
    .local-spec-grid,
    .local-prefs-row {
      grid-template-columns: 1fr;
    }
  }
</style>
