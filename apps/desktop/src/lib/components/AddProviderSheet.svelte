<script lang="ts">
  import { untrack } from 'svelte';
  import Icon from './Icon.svelte';
  import { trapFocus } from '../focus';
  import type { ProviderModel, ProviderPreset } from '../ipc';

  interface Props {
    preset: ProviderPreset;
    /** Provider keys already in use — a custom endpoint must pick a fresh one. */
    existingNames?: string[];
    onClose: () => void;
    onAdded: (name: string, model: string, useNow: boolean) => void;
  }

  let { preset, existingNames = [], onClose, onAdded }: Props = $props();

  // The sheet is mounted per preset, so these seeds never need to re-sync.
  let key = $state('');
  let model = $state(untrack(() => preset.model));
  let baseUrl = $state(untrack(() => preset.base_url));
  let models = $state<ProviderModel[]>([]);
  let scanning = $state(false);
  let scanError = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let useNow = $state(true);
  let showAdvanced = $state(false);
  let keyInput = $state<HTMLInputElement | null>(null);

  /** The custom row is a form, not a catalog entry. */
  const custom = $derived(preset.group === 'custom');
  let kind = $state(untrack(() => preset.kind));
  let name = $state('');
  let nameTouched = $state(false);

  const KINDS = [
    { id: 'openai_compat', label: 'OpenAI-compatible' },
    { id: 'anthropic', label: 'Anthropic-compatible' },
  ] as const;

  /** `https://api.silkcompute.com/v1` → `silkcompute`. */
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

  // Suggest a name from the URL until the user edits it themselves.
  $effect(() => {
    if (!custom || nameTouched) return;
    const suggested = slugFromUrl(baseUrl);
    if (suggested && suggested !== name) name = suggested;
  });

  const needsKey = $derived(preset.auth === 'api_key');
  const nameTaken = $derived(
    custom && existingNames.some((n) => n.toLowerCase() === name.trim().toLowerCase()),
  );
  const nameInvalid = $derived(
    custom && name.trim() !== '' && !/^[a-z0-9._-]+$/.test(name.trim()),
  );
  const endpointMissing = $derived(
    custom && !/^https?:\/\/[^/]/.test(baseUrl.trim()),
  );
  // A key is not required: the environment variable counts, and the provider
  // row can take a key later.
  const canAdd = $derived(
    model.trim().length > 0 &&
      (!custom || (name.trim().length > 0 && !nameTaken && !nameInvalid && !endpointMissing)),
  );

  $effect(() => {
    keyInput?.focus();
  });

  async function scan(): Promise<void> {
    scanning = true;
    scanError = null;
    models = [];
    try {
      const m = await import('../ipc');
      const list = await m.providerPreviewModels(custom ? kind : preset.kind, baseUrl, key);
      models = list
        .filter((x) => !/embed|bge-|e5-|gte-|voyage|paraphrase|rerank|:batch/i.test(x.id))
        .sort((a, b) => a.id.localeCompare(b.id));
      if (models.length === 0) {
        scanError = 'This endpoint returned no models.';
      } else if (!models.some((x) => x.id === model)) {
        const preferred =
          models.find((x) => x.id === preset.model) ??
          models.find((x) => /sonnet|gpt-5|pro|large|chat/i.test(x.id)) ??
          models.at(0);
        if (preferred) model = preferred.id;
      }
    } catch (e) {
      scanError = e instanceof Error ? e.message : String(e);
    } finally {
      scanning = false;
    }
  }

  async function add(): Promise<void> {
    busy = true;
    error = null;
    try {
      const m = await import('../ipc');
      const provider = custom ? name.trim() : preset.name;
      await m.providerUpsert(provider, custom ? kind : preset.kind, baseUrl.trim(), model.trim());
      if (needsKey && key.trim()) await m.authSetKey(provider, key.trim());
      onAdded(provider, model.trim(), useNow);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      busy = false;
    }
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onClose();
    } else if (e.key === 'Enter' && canAdd && !busy) {
      void add();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="scrim" role="presentation" onclick={onClose}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-label={`Add ${preset.label}`} use:trapFocus>
  <header class="sheet-head">
    <div>
      <p class="eyebrow mono">add provider</p>
      <h3 class="sheet-title">{preset.label}</h3>
    </div>
    <button class="icon-btn" onclick={onClose} aria-label="Close">
      <Icon name="x" size={11} />
    </button>
  </header>

  {#if custom}
    <label class="field">
      <span class="field-label">Name</span>
      <input
        class="input mono"
        type="text"
        placeholder="silkcompute"
        spellcheck="false"
        bind:value={name}
        oninput={() => (nameTouched = true)}
      />
      <span class="field-hint">
        {#if nameTaken}
          <span class="bad">Already in use — pick another name.</span>
        {:else if nameInvalid}
          <span class="bad">Lowercase letters, digits, dashes and dots only.</span>
        {:else}
          How this endpoint appears in the provider list and in /model.
        {/if}
      </span>
    </label>

    <label class="field">
      <span class="field-label">Base URL</span>
      <input
        class="input mono"
        type="text"
        placeholder="https://api.example.com/v1"
        spellcheck="false"
        bind:value={baseUrl}
      />
      <span class="field-hint">
        {#if endpointMissing && baseUrl.trim()}
          <span class="bad">Must be an https:// or http:// URL with a host.</span>
        {:else}
          Stop at the version segment — the app appends /chat/completions or
          /messages itself.
        {/if}
      </span>
    </label>

    <div class="field">
      <span class="field-label">Wire format</span>
      <div class="kinds" role="group" aria-label="Wire format">
        {#each KINDS as k (k.id)}
          <button
            class="kind-btn"
            class:on={kind === k.id}
            aria-pressed={kind === k.id}
            onclick={() => (kind = k.id)}
          >
            {k.label}
          </button>
        {/each}
      </div>
    </div>
  {/if}

  {#if needsKey}
    <label class="field">
      <span class="field-label">
        API key
        {#if preset.key_url}
          <a class="field-link" href={preset.key_url} target="_blank" rel="noreferrer">
            get one ↗
          </a>
        {/if}
      </span>
      <div class="key-row">
        <input
          class="input"
          type="password"
          placeholder={preset.env_hint ?? 'paste your key…'}
          bind:this={keyInput}
          bind:value={key}
        />
        <button
          class="btn"
          onclick={scan}
          disabled={scanning || !baseUrl.trim() || (!custom && !key.trim())}
          title="Check the key and list this provider's models"
        >
          {scanning ? 'Checking…' : 'Check key'}
        </button>
      </div>
      <span class="field-hint">
        {#if scanError}
          <span class="bad">{scanError}</span>
        {:else if models.length > 0}
          <span class="good">Key works — {models.length} models available.</span>
        {:else if custom}
          Optional — endpoints that don't check a key can leave this empty.
        {:else if key.trim()}
          Stays on this machine, in the OS keychain.
        {:else}
          Leave empty to use {preset.env_hint ?? 'an environment variable'}, or add the key later.
        {/if}
      </span>
    </label>
  {:else if preset.auth === 'oauth'}
    <p class="sheet-note">
      Add it now, then use <strong>Sign in</strong> on the provider row to connect a
      subscription — or paste an API key there instead.
    </p>
  {/if}

  <label class="field">
    <span class="field-label">
      Model
      {#if models.length > 0}
        <span class="field-count mono">{models.length} found</span>
      {/if}
    </span>
    {#if models.length > 0}
      <select class="input" bind:value={model}>
        {#each models as m (m.id)}
          <option value={m.id}>{m.id}{m.name && m.name !== m.id ? ` · ${m.name}` : ''}</option>
        {/each}
      </select>
    {:else}
      <input class="input" type="text" bind:value={model} spellcheck="false" />
    {/if}
    <span class="field-hint">
      {#if models.length > 0}
        Pick the model this provider should start on.
      {:else if custom}
        Type the model id this endpoint serves — check the key to list them.
      {:else}
        Default for {preset.label}{#if needsKey} — check the key to load the real list{/if}.
      {/if}
    </span>
  </label>

  {#if !custom}
    <button class="advanced-toggle" onclick={() => (showAdvanced = !showAdvanced)}>
      <Icon name={showAdvanced ? 'chevron-down' : 'chevron-right'} size={10} />
      Advanced
    </button>
    {#if showAdvanced}
      <label class="field">
        <span class="field-label">Base URL</span>
        <input class="input mono" type="text" bind:value={baseUrl} spellcheck="false" />
        <span class="field-hint">
          Change this for a proxy, a regional endpoint, or a self-hosted server.
        </span>
      </label>
    {/if}
  {/if}

  <label class="use-now">
    <input type="checkbox" bind:checked={useNow} />
    <span>Switch to this provider now</span>
  </label>

  {#if error}
    <p class="bad sheet-error">{error}</p>
  {/if}

  <footer class="sheet-foot">
    <button class="btn ghost" onclick={onClose} disabled={busy}>Cancel</button>
    <button class="btn primary" onclick={add} disabled={!canAdd || busy}>
      {busy ? 'Adding…' : custom ? 'Add endpoint' : `Add ${preset.label}`}
    </button>
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, #000 55%, transparent);
    backdrop-filter: blur(2px);
    z-index: 40;
  }
  .sheet {
    position: fixed;
    z-index: 41;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(440px, calc(100vw - 32px));
    max-height: calc(100vh - 64px);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px 20px 16px;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: 16px;
    box-shadow: var(--el-3);
  }
  .sheet-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }
  .eyebrow {
    font-size: 10px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--fg-3);
    margin: 0 0 4px;
  }
  .sheet-title {
    font-size: 16px;
    font-weight: 600;
    margin: 0;
    color: var(--fg-0);
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 7px;
    color: var(--fg-3);
    flex: none;
  }
  .icon-btn:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .field-label {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 11.5px;
    font-weight: 500;
    color: var(--fg-1);
  }
  .field-link {
    font-size: 10.5px;
    font-weight: 400;
    color: var(--accent);
    text-decoration: none;
  }
  .field-link:hover {
    text-decoration: underline;
  }
  .field-count {
    font-size: 10px;
    color: var(--fg-3);
  }
  .key-row {
    display: flex;
    gap: 6px;
  }
  .key-row .input {
    flex: 1 1 auto;
    min-width: 0;
  }
  .field-hint {
    font-size: 10.5px;
    line-height: 1.5;
    color: var(--fg-3);
  }
  .kinds {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    border-radius: 10px;
    background: var(--bg-2);
    align-self: flex-start;
  }
  .kind-btn {
    height: 26px;
    padding: 0 12px;
    border-radius: 7px;
    font-size: 11.5px;
    color: var(--fg-2);
  }
  .kind-btn:hover {
    color: var(--fg-0);
  }
  .kind-btn.on {
    background: var(--bg-0);
    color: var(--fg-0);
    box-shadow: var(--el-1);
  }
  .good {
    color: var(--ok, #4ade80);
  }
  .bad {
    color: var(--err);
  }
  .sheet-note {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.6;
    color: var(--fg-2);
  }
  .advanced-toggle {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    align-self: flex-start;
    margin-top: -4px;
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .advanced-toggle:hover {
    color: var(--fg-1);
  }
  .use-now {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11.5px;
    color: var(--fg-2);
    cursor: pointer;
    user-select: none;
  }
  .sheet-error {
    margin: 0;
    font-size: 11px;
  }
  .sheet-foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding-top: 4px;
  }
</style>
