<script lang="ts">
  // Landing — pick a surface. Three cards; the chat card shows live
  // provider connection state so an unconnected user knows where to go.
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import type { ProviderInfo } from '../ipc';

  let providers = $state<ProviderInfo[]>([]);
  let inTauri = $state(false);

  onMount(async () => {
    try {
      const m = await import('../ipc');
      providers = await m.providersList();
      inTauri = true;
    } catch {
      /* fixture mode — cards still work */
    }
  });

  const connected = $derived(providers.filter((p) => p.auth_kind !== 'none'));
  const defaultProvider = $derived(
    providers.find((p) => p.is_default) ?? providers[0],
  );

  const cards = $derived([
    {
      mode: 'code' as const,
      icon: 'code',
      title: 'Code',
      blurb:
        'Workspace agent — attaches a folder, reads and edits code, runs checks, verifies before it claims done.',
      foot: session.workspacePath
        ? session.workspacePath.split(/[\\/]/).filter(Boolean).pop()
        : 'no folder attached',
    },
    {
      mode: 'chat' as const,
      icon: 'chat',
      title: 'Chat',
      blurb:
        'Plain conversation with your connected models — no tools, no workspace, just talk.',
      foot: inTauri
        ? connected.length > 0
          ? `${defaultProvider?.name ?? 'provider'} · ${defaultProvider?.model ?? ''}`.trim()
          : 'no provider connected'
        : 'fixture mode',
    },
    {
      mode: 'settings' as const,
      icon: 'gear',
      title: 'Settings',
      blurb:
        'Providers and connections, workspace folder, and app details.',
      foot: `${providers.length} provider${providers.length === 1 ? '' : 's'} configured`,
    },
  ]);
</script>

<div class="landing">
  <div class="hero">
    <div class="wordmark">hate-agent<span class="cursor" aria-hidden="true"></span></div>
    <p class="tagline">
      A coding agent that spends fewer tokens per task — small prompt, lazy
      tools, shaped output, verified work.
    </p>
  </div>

  <div class="cards">
    {#each cards as card (card.mode)}
      <button class="card" onclick={() => session.setMode(card.mode)}>
        <span class="card-icon"><Icon name={card.icon} size={22} /></span>
        <span class="card-title">{card.title}</span>
        <span class="card-blurb">{card.blurb}</span>
        <span class="card-foot mono">{card.foot}</span>
      </button>
    {/each}
  </div>

  {#if inTauri && connected.length === 0}
    <button class="connect-hint" onclick={() => session.setMode('settings')}>
      <Icon name="link" size={13} />
      No provider connected — open Settings to link Nous Portal
    </button>
  {/if}
</div>

<style>
  .landing {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 24px;
    overflow-y: auto;
  }
  .hero {
    text-align: center;
    margin-bottom: 36px;
  }
  .wordmark {
    font-size: 30px;
    font-weight: 800;
    letter-spacing: -0.03em;
    color: var(--fg-0);
    margin-bottom: 10px;
  }
  .cursor {
    display: inline-block;
    width: 3px;
    height: 0.85em;
    margin-left: 3px;
    vertical-align: -0.08em;
    background: var(--accent);
    animation: blink 1.15s steps(1) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .tagline {
    font-size: 13.5px;
    line-height: 1.6;
    color: var(--fg-2);
    max-width: 460px;
    margin: 0 auto;
  }
  .cards {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    justify-content: center;
    max-width: 760px;
  }
  .card {
    width: 224px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 18px;
    text-align: left;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-sm);
    transition:
      border-color var(--motion),
      transform var(--motion),
      box-shadow var(--motion);
  }
  .card:hover {
    border-color: var(--accent-dim);
    transform: translateY(-2px);
    box-shadow: var(--shadow-md);
  }
  .card:active {
    transform: translateY(0);
  }
  .card-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    border-radius: var(--radius);
    background: var(--accent-soft);
    color: var(--accent);
    margin-bottom: 2px;
  }
  .card-title {
    font-size: 15px;
    font-weight: 700;
    color: var(--fg-0);
  }
  .card-blurb {
    font-size: 12px;
    line-height: 1.55;
    color: var(--fg-2);
    flex: 1;
  }
  .card-foot {
    font-size: 10.5px;
    color: var(--fg-3);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .connect-hint {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    margin-top: 26px;
    padding: 8px 14px;
    font-size: 12px;
    color: var(--warn);
    background: var(--accent-soft);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    transition: border-color var(--motion);
  }
  .connect-hint:hover {
    border-color: var(--warn);
  }
</style>
