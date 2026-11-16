<script lang="ts">
  import { tick } from 'svelte';
  import { session } from '../store.svelte';
  import Transcript from './Transcript.svelte';
  import Composer from './Composer.svelte';
  import PermissionBar from './PermissionBar.svelte';
  import TurnStatus from './TurnStatus.svelte';
  import StatusLine from './StatusLine.svelte';
  import Icon from './Icon.svelte';
  import FileViewer from './FileViewer.svelte';
  import PixelCat from './PixelCat.svelte';
  import { fade } from 'svelte/transition';
  import { ms } from '../motion';

  const folder = $derived(!!session.workspacePath);
  const viewing = $derived(folder && !!session.viewer);
  const fresh = $derived(!viewing && session.blocks.length === 0 && !session.sessionBusy);
  const workspaceName = $derived(
    session.workspacePath?.split(/[\\/]/).filter(Boolean).pop() ?? null,
  );

  const greeting = $derived(
    workspaceName ? `What are we building in ${workspaceName}?` : "What's on your mind?",
  );

  const STARS = Array.from({ length: 28 }, (_, i) => {
    const r = (n: number) => ((Math.sin(i * 12.9898 + n * 78.233) * 43758.5453) % 1 + 1) % 1;
    return {
      x: 3 + r(1) * 94,
      y: 4 + r(2) * 58,
      big: r(3) > 0.82,
      delay: r(4) * 4,
      dur: 2.4 + r(5) * 2.6,
    };
  });

  const SUGGESTIONS = {
    code: [
      { label: 'Explain this codebase', text: 'Give me a tour of this codebase: how it is structured and where the important pieces live.' },
      { label: 'Find and fix a bug', text: 'Look for a likely bug in this project, explain it, then fix it and verify the fix.' },
      { label: 'Review my changes', text: 'Review my uncommitted changes and point out anything risky or unfinished.' },
      { label: 'Write tests', text: 'Add tests for the code I changed most recently, then run them.' },
    ],
    chat: [
      { label: 'Explain something', text: 'Explain this to me simply: ' },
      { label: 'Help me write', text: 'Help me write ' },
      { label: 'Brainstorm ideas', text: 'Brainstorm ideas for ' },
      { label: 'Debug an error', text: 'Help me understand this error: ' },
    ],
  } as const;

  async function suggest(text: string): Promise<void> {
    session.draft = text;
    await tick();
    const ta = document.querySelector<HTMLTextAreaElement>('.composer textarea');
    if (!ta) return;
    ta.focus();
    ta.setSelectionRange(text.length, text.length);
    ta.dispatchEvent(new Event('input'));
  }
</script>

<div class="conv glass-pane" class:fresh>
  {#if fresh}
    <div class="sky" aria-hidden="true">
      {#each STARS as st, i (i)}
        <span
          class="star"
          class:big={st.big}
          style:left="{st.x}%"
          style:top="{st.y}%"
          style:animation-delay="{st.delay}s"
          style:animation-duration="{st.dur}s"
        ></span>
      {/each}
    </div>
  {/if}
  {#if session.sessionBusy}
    <div class="busy"><PixelCat size={48} zzz={false} /> <span>Opening…</span></div>
  {:else if viewing}
    <FileViewer />
  {:else if fresh}
    <div class="hello">
      <h1 class="greet">{greeting}</h1>
      {#if session.providers.length === 0}
        <button class="callout" onclick={() => session.openSettings('providers')}>
          <Icon name="key" size={14} />
          <span>Connect a provider to start — OpenRouter, Claude, Gemini, a local model…</span>
          <Icon name="chevron-right" size={13} />
        </button>
      {/if}
    </div>
  {:else}
    <Transcript />
  {/if}

  {#if session.dropping}
    <div class="drop" transition:fade={{ duration: ms(120) }}>
      <div class="drop-card">
        <Icon name="paperclip" size={22} />
        <p class="drop-title">Drop to attach</p>
        <p class="drop-sub">
          Images go with your next message.
          {session.workspacePath ? 'A folder replaces this chat’s folder.' : 'A folder opens in this chat.'}
        </p>
      </div>
    </div>
  {/if}

  {#if !session.sessionBusy}
    <div class="dock">
      <TurnStatus />
      <PermissionBar />
      <Composer />
      {#if fresh}
        <div class="chips">
          {#each SUGGESTIONS[folder ? 'code' : 'chat'] as s (s.label)}
            <button class="chip" onclick={() => void suggest(s.text)}>{s.label}</button>
          {/each}
        </div>
      {:else}
        <StatusLine />
      {/if}
    </div>
  {/if}
</div>

<style>
  .conv {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-0);
  }
  .sky {
    position: absolute;
    inset: 0 0 35% 0;
    pointer-events: none;
    background: radial-gradient(
      90% 70% at 50% 0%,
      color-mix(in srgb, var(--accent) 7%, transparent),
      transparent 72%
    );
    mask-image: linear-gradient(180deg, #000 55%, transparent);
  }
  .star {
    display: var(--sky-stars, block);
    position: absolute;
    width: 2px;
    height: 2px;
    background: var(--fg-1);
    opacity: 0.15;
    animation: twinkle ease-in-out infinite;
  }
  .star.big {
    width: 3px;
    height: 3px;
    box-shadow: 0 0 6px color-mix(in srgb, var(--fg-0) 40%, transparent);
  }
  @keyframes twinkle {
    50% {
      opacity: 0.7;
    }
  }
  :global(:root[data-theme='light']) .star {
    display: none;
  }
  @media (prefers-color-scheme: light) {
    :global(:root:not([data-theme])) .star {
      display: none;
    }
  }
  :global(:root[data-ambient-motion='off']) .star {
    animation: none;
    opacity: 0.35;
  }
  .hello,
  .dock,
  .busy {
    position: relative;
  }
  .drop {
    position: absolute;
    inset: 10px;
    z-index: 40;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
    background: color-mix(in srgb, var(--bg-0) 78%, transparent);
    backdrop-filter: blur(3px);
    border: 2px dashed color-mix(in srgb, var(--accent) 55%, var(--border-strong));
    border-radius: 18px;
  }
  .drop-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    color: var(--accent);
    text-align: center;
  }
  .drop-title {
    margin: 4px 0 0;
    font-family: 'Alagard', var(--font-ui);
    font-size: 22px;
    color: var(--fg-0);
  }
  .drop-sub {
    margin: 0;
    max-width: 340px;
    font-size: 12.5px;
    color: var(--fg-2);
  }
  .conv.fresh {
    justify-content: center;
    padding-bottom: 8vh;
  }
  .hello {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    padding: 0 24px 22px;
    text-align: center;
  }
  .greet {
    margin: 0;
    font-family: 'Alagard', var(--font-ui);
    font-weight: 400;
    font-size: clamp(24px, 3.4vw, 34px);
    letter-spacing: 0.02em;
    color: var(--fg-0);
  }
  .callout {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    max-width: 520px;
    padding: 8px 12px;
    font-size: 12.5px;
    color: var(--fg-1);
    text-align: left;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: 12px;
  }
  .callout:hover {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border-strong));
    color: var(--fg-0);
  }
  .dock {
    flex: none;
    width: 100%;
    max-width: 800px;
    margin: 0 auto;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    padding: 6px 16px 0;
  }
  .chip {
    padding: 7px 14px;
    font-size: 12.5px;
    color: var(--fg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    transition: background var(--motion), color var(--motion), border-color var(--motion);
  }
  .chip:hover {
    background: var(--bg-1);
    color: var(--fg-0);
    border-color: var(--border-strong);
  }
  .busy {
    margin: auto;
    display: inline-flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--fg-3);
  }
</style>
