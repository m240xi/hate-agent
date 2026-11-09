<script lang="ts">
  import { session } from '../store.svelte';
  import Popover from './Popover.svelte';
  import Icon from './Icon.svelte';
  import type { ThinkingLevel } from '../ipc';

  const OPTIONS: { value: ThinkingLevel; label: string; hint: string }[] = [
    { value: 'off', label: 'auto', hint: 'provider default effort' },
    { value: 'low', label: 'low', hint: 'light reasoning' },
    { value: 'medium', label: 'med', hint: 'moderate reasoning' },
    { value: 'high', label: 'high', hint: 'full reasoning budget' },
  ];

  const LEVEL_EFFORTS: Record<Exclude<ThinkingLevel, 'off'>, string[]> = {
    low: ['low', 'minimal'],
    medium: ['medium'],
    high: ['high', 'xhigh', 'max'],
  };

  const efforts = $derived(session.selectedModel?.reasoning_efforts ?? []);
  const effortsKnown = $derived(efforts.length > 0);

  function allowed(level: ThinkingLevel): boolean {
    if (level === 'off' || !effortsKnown) return true;
    const candidates = LEVEL_EFFORTS[level as Exclude<ThinkingLevel, 'off'>];
    return candidates.some((e) => efforts.includes(e));
  }

  const currentLabel = $derived(
    OPTIONS.find((o) => o.value === session.thinkingLevel)?.label ?? 'auto',
  );
</script>

<Popover align="left" width="240px">
  {#snippet trigger()}
    <span class="tm-trigger-label">{currentLabel}</span>
    <Icon name="chevron-down" size={10} />
  {/snippet}
  {#snippet panel({ close })}
    <div class="tm" role="listbox" aria-label="Thinking level">
      <div class="tm-head mono">thinking effort</div>
      {#each OPTIONS as o (o.value)}
        {@const ok = allowed(o.value)}
        <button
          type="button"
          class="tm-row"
          class:selected={o.value === session.thinkingLevel}
          class:disabled={!ok}
          role="option"
          aria-selected={o.value === session.thinkingLevel}
          aria-disabled={!ok}
          title={ok
            ? o.hint
            : `unsupported by ${session.model} (accepts: ${efforts.join(', ')})`}
          onclick={() => {
            if (!ok) return;
            session.setThinking(o.value);
            close();
          }}
        >
          <span class="tm-label">{o.label}</span>
          <span class="tm-hint">{ok ? o.hint : 'unsupported'}</span>
          {#if o.value === session.thinkingLevel}
            <span class="tm-check"><Icon name="check" size={11} /></span>
          {/if}
        </button>
      {/each}
      {#if effortsKnown}
        <div class="tm-foot mono">
          {session.model} accepts: {efforts.join(' · ')}
        </div>
      {/if}
    </div>
  {/snippet}
</Popover>

<style>
  .tm-trigger-label {
    min-width: 34px;
    text-align: left;
  }
  .tm {
    display: flex;
    flex-direction: column;
    padding: 4px;
  }
  .tm-head {
    padding: 6px 10px 5px;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .tm-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    border-radius: var(--radius-sm);
    text-align: left;
    cursor: pointer;
  }
  .tm-row:hover:not(.disabled) {
    background: var(--bg-2);
  }
  .tm-row.selected {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-1));
  }
  .tm-row.disabled {
    opacity: 0.45;
    cursor: default;
  }
  .tm-label {
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-0);
    width: 44px;
    flex: none;
  }
  .tm-row.selected .tm-label {
    color: var(--accent);
  }
  .tm-hint {
    flex: 1;
    min-width: 0;
    font-size: 10.5px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tm-check {
    flex: none;
    color: var(--accent);
    display: inline-flex;
  }
  .tm-foot {
    padding: 6px 10px 5px;
    font-size: 9.5px;
    color: var(--fg-3);
    border-top: 1px solid var(--border);
    margin-top: 3px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
