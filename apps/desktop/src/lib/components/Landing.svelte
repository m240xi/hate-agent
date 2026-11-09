<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import NightScene from './NightScene.svelte';
  import type { ProviderInfo } from '../ipc';

  let providers = $state<ProviderInfo[]>([]);
  let version = $state('0.1.0');
  let inTauri = $state(false);

  onMount(async () => {
    try {
      const m = await import('../ipc');
      providers = await m.providersList();
      version = (await m.appInfo()).version;
      inTauri = true;
    } catch {
    }
  });

  const connected = $derived(providers.some((p) => p.auth_kind !== 'none'));
  const primary = $derived(
    providers.find((p) => p.is_default) ?? providers[0],
  );

  const PLATES = [
    { mode: 'code' as const, label: 'Code' },
    { mode: 'chat' as const, label: 'Chat' },
    { mode: 'settings' as const, label: 'Settings' },
  ];
</script>

<div class="landing">
  <NightScene />

  <div class="hero">
    <div class="version pixel">v{version}</div>
    <h1 class="wordmark">hate-agent</h1>
    <p class="tagline pixel">
      A coding agent that spends fewer tokens per task — small prompt,
      lazy tools, verified work. Built for the 3am club.
    </p>

    <div class="plates">
      {#each PLATES as plate (plate.mode)}
        <button class="plate pixel" onclick={() => session.setMode(plate.mode)}>
          {plate.label}
        </button>
      {/each}
    </div>

    {#if inTauri && !connected}
      <button class="connect-hint" onclick={() => session.setMode('settings')}>
        <Icon name="link" size={13} />
        No provider connected — Settings to link Nous Portal
      </button>
    {/if}
  </div>

  {#if inTauri && connected && primary}
    <div class="status pixel">{primary.name} · {primary.model}</div>
  {/if}
</div>

<style>
  .landing {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .hero {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 24px;
    text-align: center;
  }
  .pixel {
    font-family: 'm5x7', var(--font-mono);
    font-size: 16px;
    letter-spacing: 0.02em;
  }
  .version {
    color: rgba(240, 238, 233, 0.6);
    margin-bottom: 18px;
  }
  .wordmark {
    font-family: 'Alagard', var(--font-ui);
    font-size: 62px;
    font-weight: 400;
    color: #f0eee9;
    margin: 0 0 14px;
    line-height: 1;
    text-shadow: 0 3px 0 rgba(0, 0, 0, 0.9), 0 8px 24px rgba(0, 0, 0, 0.8);
  }
  .tagline {
    color: rgba(240, 238, 233, 0.75);
    max-width: 560px;
    margin: 0 auto 30px;
    line-height: 1.5;
    text-shadow: 0 1px 0 rgba(0, 0, 0, 0.9);
  }

  .plates {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    justify-content: center;
  }
  .plate {
    font-family: 'Alagard', var(--font-ui);
    font-size: 19px;
    letter-spacing: 0.05em;
    color: #e9e7e1;
    padding: 12px 38px;
    min-width: 136px;
    background: linear-gradient(#1a1c1f, #121315);
    border: 2px solid #2b2d31;
    border-radius: 6px;
    box-shadow:
      inset 0 2px 0 rgba(255, 255, 255, 0.07),
      inset 0 -2px 0 rgba(0, 0, 0, 0.55),
      0 4px 0 #050505,
      var(--shadow-md);
    text-shadow: 0 2px 0 rgba(0, 0, 0, 0.65);
    cursor: pointer;
    transition:
      transform var(--motion),
      border-color var(--motion),
      box-shadow var(--motion);
  }
  .plate:hover {
    border-color: var(--accent-dim);
    color: #ffffff;
    transform: translateY(-1px);
    box-shadow:
      inset 0 2px 0 rgba(255, 255, 255, 0.1),
      inset 0 -2px 0 rgba(0, 0, 0, 0.55),
      0 5px 0 #050505,
      var(--shadow-md);
  }
  .plate:active {
    transform: translateY(3px);
    box-shadow:
      inset 0 2px 0 rgba(255, 255, 255, 0.05),
      inset 0 -1px 0 rgba(0, 0, 0, 0.55),
      0 1px 0 #050505,
      var(--shadow-sm);
  }

  .connect-hint {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    margin-top: 30px;
    padding: 8px 14px;
    font-size: 12px;
    color: var(--warn);
    background: rgba(10, 10, 12, 0.7);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    transition: border-color var(--motion);
    backdrop-filter: blur(4px);
  }
  .connect-hint:hover {
    border-color: var(--warn);
  }

  .status {
    position: absolute;
    bottom: 14px;
    left: 18px;
    color: rgba(240, 238, 233, 0.45);
    font-size: 14px;
    text-shadow: 0 1px 0 #000;
  }
</style>
