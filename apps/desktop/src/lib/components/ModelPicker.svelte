<script lang="ts">
  import { session } from '../store.svelte';
  import Popover from './Popover.svelte';
  import Icon from './Icon.svelte';
  import type { ProviderModel } from '../ipc';

  // Model picker: searchable, vendor-grouped catalog with pricing and
  // context metadata, plus a session-stats footer (ctx fill, turns,
  // elapsed, cumulative tokens) — the Claude-Code-style status line.
  let query = $state('');
  let highlightId = $state('');
  let panelOpen = $state(false);
  let now = $state(Date.now());

  interface VendorGroup {
    title: string;
    items: ProviderModel[];
  }

  interface Section {
    /** Provider config key (`nous`, `claude`, …) — stable `#each` key. */
    key: string;
    /** Human label for the provider header. */
    label: string;
    count: number;
    vendors: VendorGroup[];
  }

  /** Friendly provider names — the picker's top-level sections. */
  const PROVIDER_LABELS: Record<string, string> = {
    nous: 'Hermes',
    claude: 'Claude',
    anthropic: 'Claude',
    openai: 'OpenAI',
    openrouter: 'OpenRouter',
    deepseek: 'DeepSeek',
    xai: 'xAI',
    grok: 'xAI',
    gemini: 'Gemini',
    google: 'Gemini',
    groq: 'Groq',
    mistral: 'Mistral',
    ollama: 'Ollama',
    lmstudio: 'LM Studio',
  };

  function providerLabel(name: string): string {
    const key = name.toLowerCase();
    return (
      PROVIDER_LABELS[key] ??
      key.charAt(0).toUpperCase() + key.slice(1)
    );
  }

  /** Aggregator catalogs prefix the vendor: "Anthropic: Claude Fable
   *  Latest". The picker shows just the model name — the id line
   *  underneath still carries the full slug for disambiguation. */
  const VENDOR_PREFIX =
    /^(anthropic|openai|google|deepmind|meta|mistral|xai|deepseek|qwen|moonshot(?:ai)?|z\.?ai|zhipu|minimax|nous|openrouter|amazon|aws|nvidia|cohere|ai21|databricks|snowflake|ibm|writer|inflection)\s*:\s*/i;

  function displayName(m: ProviderModel): string {
    const n = m.name ?? m.id;
    return n.replace(VENDOR_PREFIX, '') || n;
  }

  /** Vendor family from an aggregator id (`anthropic/x` → `anthropic`).
   *  A leading `~` marks "latest" aliases and folds into its family. */
  function vendorOf(m: ProviderModel): string {
    const id = m.id.replace(/^~/, '');
    const slash = id.indexOf('/');
    return slash > 0 ? id.slice(0, slash) : '';
  }

  // Sections are providers first, vendor families second — so Hermes's
  // anthropic/openai/deepseek models live under one "Hermes" header
  // instead of masquerading as providers. Single-vendor catalogs (a
  // plain Anthropic key) skip the sub-header and render flat.
  const sections = $derived.by<Section[]>(() => {
    const q = query.trim().toLowerCase();
    const byProvider = new Map<string, Map<string, ProviderModel[]>>();
    for (const m of session.models) {
      const hay = `${m.id} ${m.name ?? ''} ${m.provider ?? ''}`.toLowerCase();
      if (q && !hay.includes(q)) continue;
      const vendor = vendorOf(m);
      const provider = m.provider || vendor || 'other';
      const vendors = byProvider.get(provider) ?? new Map<string, ProviderModel[]>();
      const list = vendors.get(vendor) ?? [];
      list.push(m);
      vendors.set(vendor, list);
      byProvider.set(provider, vendors);
    }
    const current = session.providerName;
    return [...byProvider.entries()]
      .map(([key, vendors]) => {
        const groups = [...vendors.entries()]
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([title, items]) => ({
            title,
            items: items.sort((a, b) => a.id.localeCompare(b.id)),
          }));
        return {
          key,
          label: providerLabel(key),
          count: groups.reduce((n, g) => n + g.items.length, 0),
          vendors: groups,
        };
      })
      .sort((a, b) => {
        // The session's provider first, then alphabetical by label.
        const da = a.key === current ? 0 : 1;
        const db = b.key === current ? 0 : 1;
        return da - db || a.label.localeCompare(b.label);
      });
  });

  const flat = $derived(sections.flatMap((s) => s.vendors.flatMap((v) => v.items)));

  // Keep the keyboard highlight on the first match whenever the
  // filtered list changes.
  $effect(() => {
    void query;
    highlightId = flat[0]?.id ?? '';
  });

  // Live elapsed clock — only ticks while the panel is open.
  $effect(() => {
    if (!panelOpen || session.sessionBeganAt === null) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  const elapsed = $derived(
    session.sessionBeganAt === null ? '—' : fmtElapsed(now - session.sessionBeganAt),
  );

  function fmtElapsed(ms: number): string {
    const s = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    const mm = String(m).padStart(2, '0');
    const ss = String(sec).padStart(2, '0');
    return h > 0 ? `${h}:${mm}:${ss}` : `${m}:${ss}`;
  }

  function fmtTokens(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }

  function fmtCtx(n: number | null | undefined): string {
    if (n == null) return '—';
    if (n >= 1_000_000) {
      const m = n / 1_000_000;
      return `${m >= 10 ? Math.round(m) : m.toFixed(1)}M`;
    }
    if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
    return String(n);
  }

  /** USD per 1M tokens → compact price; null = unpriced/unknown. */
  function fmtPrice(v: number | null | undefined): string {
    if (v == null) return '—';
    if (v === 0) return 'free';
    if (v >= 0.01) return `$${v.toFixed(2)}`;
    return `$${v.toFixed(4).replace(/0+$/, '').replace(/\.$/, '')}`;
  }

  const triggerLabel = $derived.by(() => {
    if (session.modelsState === 'loading' && session.models.length === 0)
      return 'loading models…';
    if (session.modelsState === 'error' && session.models.length === 0)
      return 'model list failed';
    const m = session.selectedModel;
    return m ? displayName(m) : (session.model ?? 'model');
  });

  const triggerTitle = $derived(
    session.modelsState === 'error'
      ? `model list failed: ${session.modelsError}`
      : session.model,
  );

  function priceLabel(m: ProviderModel): string {
    if (m.price_in_mtok == null && m.price_out_mtok == null) return '';
    return `${fmtPrice(m.price_in_mtok)}/${fmtPrice(m.price_out_mtok)}`;
  }

  function onSearchKeydown(e: KeyboardEvent): void {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (flat.length === 0) return;
      const i = flat.findIndex((m) => m.id === highlightId);
      const next =
        e.key === 'ArrowDown'
          ? flat[(i + 1) % flat.length]
          : flat[(i - 1 + flat.length) % flat.length];
      highlightId = next?.id ?? highlightId;
      document
        .getElementById(`mp-row-${CSS.escape(highlightId)}`)
        ?.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const m = flat.find((x) => x.id === highlightId) ?? flat[0];
      if (m) void session.setModel(m.id, m.provider);
    }
  }
</script>

<Popover width="440px" onOpenChange={(o) => (panelOpen = o)}>
  {#snippet trigger()}
    <span class="mp-trigger-label" title={triggerTitle}>{triggerLabel}</span>
    <Icon name="chevron-down" size={10} />
  {/snippet}
  {#snippet panel({ close })}
    <div class="mp">
      <div class="mp-search">
        <input
          bind:value={query}
          onkeydown={onSearchKeydown}
          placeholder="Search models…"
          aria-label="Search models"
          autocomplete="off"
          spellcheck="false"
        />
      </div>
      <div class="mp-list" role="listbox" aria-label="Models">
        {#each sections as sec (sec.key)}
          <div class="mp-group">
            <span class="mp-group-name">{sec.label}</span>
            <span class="mp-group-count mono">{sec.count}</span>
          </div>
          {#each sec.vendors as vg (vg.title)}
            {#if sec.vendors.length > 1}
              <div class="mp-subgroup">
                <span class="mp-subgroup-name">{vg.title || 'other'}</span>
                <span class="mp-group-count mono">{vg.items.length}</span>
              </div>
            {/if}
            {#each vg.items as m (m.id)}
              <button
                id="mp-row-{m.id}"
                type="button"
                class="mp-row"
                class:selected={m.id === session.model}
                class:highlighted={m.id === highlightId}
                role="option"
                aria-selected={m.id === session.model}
                onclick={() => {
                  void session.setModel(m.id, m.provider);
                  close();
                }}
                onmouseenter={() => (highlightId = m.id)}
              >
                <span class="mp-main">
                  <span class="mp-name">{displayName(m)}</span>
                  <span class="mp-id mono">{m.id}</span>
                </span>
                <span class="mp-meta">
                  {#if m.supports_tools === true || m.supports_vision === true}
                    <span class="mp-badges">
                      {#if m.supports_tools === true}<span class="mp-badge" title="tool calls">⚒</span>{/if}
                      {#if m.supports_vision === true}<span class="mp-badge" title="vision">◉</span>{/if}
                    </span>
                  {/if}
                  {#if priceLabel(m)}
                    <span class="mp-price mono" title="$/1M in · out">{priceLabel(m)}</span>
                  {/if}
                  <span class="mp-ctx mono" title="context window">
                    {fmtCtx(m.context_tokens)}
                  </span>
                </span>
              </button>
            {/each}
          {/each}
        {/each}
        {#if flat.length === 0}
          <div class="mp-empty">no models match “{query}”</div>
        {/if}
      </div>
      <div class="mp-foot">
        <div class="mp-ctxbar" title="context window usage">
          <div class="mp-ctxfill" style:width="{session.ctxRatio * 100}%"></div>
        </div>
        <div class="mp-stats mono">
          <span title="context used / window">
            ctx {fmtTokens(session.ctxUsed)}/{fmtCtx(session.ctxLimit)}
          </span>
          <span title="turns this session">{session.sessionTurns} turns</span>
          <span title="session elapsed">{elapsed}</span>
          <span title="tokens this session">
            {fmtTokens(session.sessionTotals.input + session.sessionTotals.output)} tok
          </span>
        </div>
      </div>
    </div>
  {/snippet}
</Popover>

<style>
  .mp-trigger-label {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mp {
    display: flex;
    flex-direction: column;
    min-height: 0;
    max-height: min(480px, 70vh);
  }
  .mp-search {
    padding: 8px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .mp-search input {
    width: 100%;
    padding: 6px 9px;
    font-size: 12px;
    color: var(--fg-0);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    outline: none;
  }
  .mp-search input:focus {
    border-color: var(--accent-dim);
  }
  .mp-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 0;
  }
  .mp-group {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 8px 12px 3px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--fg-3);
    position: sticky;
    top: 0;
    background: var(--bg-1);
  }
  .mp-group-count {
    font-weight: 400;
    color: var(--fg-3);
    opacity: 0.7;
  }
  /* Vendor family inside a provider — sits under the sticky provider
     header, smaller and indented so the hierarchy reads at a glance. */
  .mp-subgroup {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 6px 12px 2px 20px;
    font-size: 9.5px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-3);
    position: sticky;
    top: 25px;
    background: var(--bg-1);
    opacity: 0.85;
  }
  .mp-subgroup-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mp-row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 6px 12px;
    text-align: left;
    cursor: pointer;
  }
  .mp-row.highlighted {
    background: var(--bg-2);
  }
  .mp-row.selected {
    background: color-mix(in srgb, var(--accent) 10%, var(--bg-1));
  }
  .mp-row.selected .mp-name {
    color: var(--accent);
  }
  .mp-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .mp-name {
    font-size: 12px;
    color: var(--fg-0);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mp-id {
    font-size: 10px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mp-meta {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 10px;
    color: var(--fg-3);
  }
  .mp-badges {
    display: inline-flex;
    gap: 3px;
    color: var(--fg-3);
  }
  .mp-badge {
    font-size: 10px;
  }
  .mp-price {
    min-width: 52px;
    text-align: right;
    color: var(--fg-2);
  }
  .mp-ctx {
    min-width: 34px;
    text-align: right;
    color: var(--fg-3);
  }
  .mp-empty {
    padding: 24px;
    text-align: center;
    font-size: 12px;
    color: var(--fg-3);
  }
  .mp-foot {
    flex: none;
    border-top: 1px solid var(--border);
    padding: 8px 12px 9px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .mp-ctxbar {
    height: 4px;
    border-radius: 2px;
    background: var(--bg-3);
    overflow: hidden;
  }
  .mp-ctxfill {
    height: 100%;
    background: var(--accent);
    transition: width 300ms ease;
  }
  .mp-stats {
    display: flex;
    gap: 12px;
    font-size: 10px;
    color: var(--fg-2);
    flex-wrap: wrap;
  }
</style>
