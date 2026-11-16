<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import NightScene from './NightScene.svelte';
  import { motionReduced } from '../motion';
  import type { ProviderInfo } from '../ipc';

  let providers = $state<ProviderInfo[]>([]);
  let version = $state('0.1.0');
  let inTauri = $state(false);

  let landingEl = $state<HTMLElement | null>(null);
  let raf = 0;
  let nextX = 0;
  let nextY = 0;

  // One rAF-coalesced write per pointer move; a no-op when decorative motion is
  // settled, so the scene never fights a reduced-motion preference.
  function trackPointer(e: PointerEvent): void {
    if (motionReduced()) return;
    const el = landingEl;
    if (!el) return;
    const r = el.getBoundingClientRect();
    nextX = ((e.clientX - r.left) / r.width - 0.5) * 2;
    nextY = ((e.clientY - r.top) / r.height - 0.5) * 2;
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      el.style.setProperty('--lx', nextX.toFixed(3));
      el.style.setProperty('--ly', nextY.toFixed(3));
    });
  }

  onMount(async () => {
    try {
      const m = await import('../ipc');
      providers = await m.providersList();
      version = (await m.appInfo()).version;
      inTauri = true;
      void session.refreshSessions();
    } catch {
    }
  });

  const connected = $derived(providers.some((p) => p.auth_kind !== 'none'));

  const steps = $derived([
    {
      id: 'provider',
      label: 'Connect a provider',
      hint: 'OpenRouter, OpenAI, Claude, Gemini, a local server…',
      done: connected,
      cta: 'Add one',
      run: () => session.openSettings('providers'),
    },
    {
      id: 'first',
      label: 'Start your first chat',
      hint: 'Add a folder from the composer to let it read and edit a project.',
      done: session.sessionTurns > 0 || session.sessions.length > 0,
      cta: 'Start',
      run: () => session.startFresh(),
    },
  ]);
  const remaining = $derived(steps.filter((s) => !s.done).length);
  const primary = $derived(
    providers.find((p) => p.is_default) ?? providers[0],
  );

  const PLATES = [
    { id: 'chat', label: 'Chat', run: () => session.setMode('chat') },
    {
      id: 'folder',
      label: 'Open folder',
      run: () => {
        session.startFresh();
        void session.pickWorkspace();
      },
    },
    { id: 'settings', label: 'Settings', run: () => session.setMode('settings') },
  ];
</script>

<div
  class="landing"
  role="presentation"
  onpointermove={trackPointer}
  onpointerleave={() => {
    landingEl?.style.setProperty('--lx', '0');
    landingEl?.style.setProperty('--ly', '0');
  }}
  bind:this={landingEl}
>
  <NightScene />
  <div class="aurora" aria-hidden="true"></div>

  <div class="hero">
    <div class="version pixel">v{version}</div>
    <h1 class="wordmark">hate-agent</h1>
    <p class="tagline pixel">Built for the 3am club.</p>

    <div class="plates">
      {#each PLATES as plate (plate.id)}
        <button class="plate pixel" onclick={plate.run}>
          {plate.label}
        </button>
      {/each}
    </div>

    {#if inTauri && remaining > 0}
      <div class="checklist">
        <p class="check-head pixel">first run · {remaining} step{remaining === 1 ? '' : 's'} left</p>
        <ul>
          {#each steps as step (step.id)}
            <li class="check" class:done={step.done}>
              <span class="check-mark" aria-hidden="true">
                {#if step.done}
                  <Icon name="check" size={10} />
                {:else}
                  <span class="check-dot"></span>
                {/if}
              </span>
              <span class="check-text">
                <span class="check-label">{step.label}</span>
                <span class="check-hint pixel">{step.hint}</span>
              </span>
              {#if !step.done}
                <button class="check-cta pixel" onclick={step.run}>{step.cta}</button>
              {/if}
            </li>
          {/each}
        </ul>
      </div>
    {:else if inTauri && !connected}
      <button class="connect-hint" onclick={() => session.openSettings('providers')}>
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
  /* The scene leans away from the pointer by a few pixels. Coalesced into one
     animation frame, and it simply stops writing when decorative motion is off
     (the explicit Reduces setting, or a settled system preference). */
  .landing {
    --lx: 0;
    --ly: 0;
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
  /* the palette's own accent, breathing over the horizon: the night scene and
     the UI are lit by the same colour */
  .aurora {
    position: absolute;
    left: 50%;
    bottom: -8%;
    width: min(1200px, 130%);
    height: 48%;
    transform: translateX(-50%);
    pointer-events: none;
    background: radial-gradient(
      62% 100% at 50% 100%,
      color-mix(in srgb, var(--accent) 26%, transparent),
      transparent 70%
    );
    mix-blend-mode: screen;
    opacity: 0.45;
    animation: aurora-breathe 17s steps(24, end) infinite alternate;
  }
  @keyframes aurora-breathe {
    from {
      opacity: 0.26;
      transform: translateX(-50%) scaleY(0.92);
    }
    to {
      opacity: 0.52;
      transform: translateX(-50%) scaleY(1.06);
    }
  }
  :global(:root[data-ambient-motion='off']) .aurora {
    animation: none;
    opacity: 0.34;
    transform: translateX(-50%);
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
    color: color-mix(in srgb, var(--accent) 55%, #f0eee9);
    margin: 0 0 14px;
    line-height: 1;
    text-shadow: 0 3px 0 rgba(0, 0, 0, 0.45);
    text-shadow: 0 3px 0 rgba(0, 0, 0, 0.9), 0 8px 24px rgba(0, 0, 0, 0.8);
  }
  .tagline {
    color: rgba(240, 238, 233, 0.75);
    max-width: 560px;
    margin: 0 auto 30px;
    line-height: 1.5;
    text-shadow: 0 1px 0 rgba(0, 0, 0, 0.9);
    text-wrap: pretty;
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
    background: linear-gradient(
      color-mix(in srgb, var(--accent) 6%, #1a1c1f),
      #121315
    );
    border: 2px solid color-mix(in srgb, var(--accent-dim) 45%, #2b2d31);
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

  .checklist {
    margin-top: 26px;
    width: min(460px, 100%);
    text-align: left;
    padding: 14px 16px;
    background: rgba(10, 10, 12, 0.72);
    border: 1px solid var(--border);
    border-radius: 12px;
    backdrop-filter: blur(6px);
  }
  .check-head {
    margin: 0 0 10px;
    font-size: 13px;
    color: rgba(240, 238, 233, 0.7);
  }
  .checklist ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 9px;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 9px;
  }
  .check-mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 15px;
    height: 15px;
    flex: none;
    margin-top: 1px;
    border-radius: 50%;
    border: 1px solid var(--border-strong);
    color: rgba(240, 238, 233, 0.85);
  }
  .check.done .check-mark {
    border-color: var(--ok);
    color: var(--ok);
  }
  .check-dot {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--fg-3);
  }
  .check-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .check-label {
    font-size: 12.5px;
    color: rgba(240, 238, 233, 0.92);
  }
  .check.done .check-label {
    color: rgba(240, 238, 233, 0.55);
    text-decoration: line-through;
    text-decoration-color: var(--fg-3);
  }
  .check-hint {
    font-size: 12px;
    color: rgba(240, 238, 233, 0.45);
  }
  .check-cta {
    margin-left: auto;
    flex: none;
    padding: 4px 10px;
    font-size: 12px;
    color: rgba(240, 238, 233, 0.9);
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.04);
  }
  .check-cta:hover {
    border-color: var(--accent-dim);
    background: rgba(255, 255, 255, 0.08);
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
