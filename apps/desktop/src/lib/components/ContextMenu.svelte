<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Icon from './Icon.svelte';
  import { ctxMenu, closeMenu, installContextMenu, type MenuEntry } from '../context-menu.svelte';

  let panel = $state<HTMLDivElement | null>(null);
  let left = $state(0);
  let top = $state(0);

  onMount(installContextMenu);

  function focusables(): HTMLButtonElement[] {
    if (!panel) return [];
    return [...panel.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
  }

  // Measure after paint, then keep the panel inside the window.
  $effect(() => {
    if (!ctxMenu.open || !panel) return;
    const pad = 8;
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    const maxX = Math.max(pad, window.innerWidth - w - pad);
    const maxY = Math.max(pad, window.innerHeight - h - pad);
    left = Math.min(Math.max(pad, ctxMenu.x), maxX);
    top = Math.min(Math.max(pad, ctxMenu.y), maxY);
    void tick().then(() => focusables()[0]?.focus());
  });

  function onKeyDown(event: KeyboardEvent): void {
    const items = focusables();
    if (items.length === 0) return;
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      items[(at + 1) % items.length]?.focus();
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      items[(at - 1 + items.length) % items.length]?.focus();
    } else if (event.key === 'Home') {
      event.preventDefault();
      items[0]?.focus();
    } else if (event.key === 'End') {
      event.preventDefault();
      items[items.length - 1]?.focus();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      closeMenu(true);
    } else if (event.key === 'Tab') {
      event.preventDefault();
      closeMenu();
    }
  }

  async function activate(entry: Exclude<MenuEntry, { sep: true }>): Promise<void> {
    if ('sep' in entry || entry.disabled) return;
    closeMenu();
    try {
      await entry.run();
    } catch (err) {
      console.error('context menu action failed', err);
    }
  }
</script>

{#if ctxMenu.open}
  <div class="ctx-scrim" aria-hidden="true"></div>
  <div
    class="ctx-menu"
    role="menu"
    tabindex="-1"
    aria-label="Actions"
    bind:this={panel}
    style:left="{left}px"
    style:top="{top}px"
    onkeydown={onKeyDown}
    oncontextmenu={(e) => e.preventDefault()}
  >
    {#each ctxMenu.entries as entry, i (i)}
      {#if 'sep' in entry}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          class="item"
          class:danger={entry.danger}
          role="menuitem"
          tabindex="-1"
          disabled={entry.disabled}
          onclick={() => void activate(entry)}
        >
          <span class="ico">
            {#if entry.icon}<Icon name={entry.icon} size={13} />{/if}
          </span>
          <span class="label">{entry.label}</span>
          {#if entry.hint}<span class="hint mono">{entry.hint}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .ctx-scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
  }
  .ctx-menu {
    position: fixed;
    z-index: 71;
    min-width: 208px;
    max-width: 300px;
    padding: 4px;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: 10px;
    box-shadow: var(--el-3);
    animation: menu-in var(--motion) backwards;
    outline: none;
  }
  /* in glass mode the surface tokens turn translucent, so the panel frosts its
     own backdrop instead of letting the transcript show through */
  :global(:root[data-glass]) .ctx-menu {
    backdrop-filter: blur(14px) saturate(150%);
  }
  @keyframes menu-in {
    from {
      opacity: 0;
      transform: translateY(-3px) scale(0.985);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  :global(:root[data-ambient-motion='off']) .ctx-menu {
    animation: none;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--fg-1);
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }
  .item:hover:not(:disabled),
  .item:focus-visible {
    background: var(--bg-3);
    color: var(--fg-0);
    outline: none;
  }
  .item.danger {
    color: var(--err);
  }
  .item.danger:hover:not(:disabled),
  .item.danger:focus-visible {
    background: color-mix(in srgb, var(--err) 16%, var(--bg-3));
    color: var(--err);
  }
  .item:disabled {
    color: var(--fg-3);
    cursor: default;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    flex: none;
    color: var(--fg-2);
  }
  .item:hover:not(:disabled) .ico,
  .item:focus-visible .ico {
    color: currentColor;
  }
  .label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    flex: none;
    font-size: 10px;
    color: var(--fg-3);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
</style>
