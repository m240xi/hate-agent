<script lang="ts">
  import type { Snippet } from 'svelte';

  // Generic upward-opening popover: trigger button + floating panel.
  // Closes on Esc, outside click, or when the panel calls `close()`.
  interface Props {
    trigger: Snippet;
    panel: Snippet<[{ close: () => void }]>;
    /** Called whenever the panel opens/closes (for focus/interval needs). */
    onOpenChange?: (open: boolean) => void;
    align?: 'left' | 'right';
    /** Panel width (px or css length). Height stays content-driven. */
    width?: string;
  }

  let {
    trigger,
    panel,
    onOpenChange,
    align = 'right',
    width,
  }: Props = $props();

  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);

  function setOpen(v: boolean): void {
    if (open === v) return;
    open = v;
    onOpenChange?.(v);
  }

  function toggle(): void {
    setOpen(!open);
  }

  function onWindowClick(e: MouseEvent): void {
    if (open && root && !root.contains(e.target as Node)) setOpen(false);
  }

  function onKeydown(e: KeyboardEvent): void {
    if (open && e.key === 'Escape') {
      e.stopPropagation();
      setOpen(false);
    }
  }
</script>

<svelte:window onclick={onWindowClick} onkeydowncapture={onKeydown} />

<div class="pop" class:pop-left={align === 'left'} bind:this={root}>
  <button
    type="button"
    class="pop-trigger"
    onclick={toggle}
    aria-expanded={open}
    aria-haspopup="dialog"
  >
    {@render trigger()}
  </button>
  {#if open}
    <div class="pop-panel" role="dialog" style:width={width}>
      {@render panel({ close: () => setOpen(false) })}
    </div>
  {/if}
</div>

<style>
  .pop {
    position: relative;
    display: inline-flex;
    flex: none;
  }
  .pop-trigger {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 8px;
    font-size: 10.5px;
    color: var(--fg-1);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    max-width: 240px;
    overflow: hidden;
    white-space: nowrap;
  }
  .pop-trigger:hover {
    border-color: var(--border-strong);
    color: var(--fg-0);
  }
  .pop-trigger[aria-expanded='true'] {
    border-color: var(--accent-dim);
    color: var(--fg-0);
  }
  .pop-panel {
    position: absolute;
    bottom: calc(100% + 8px);
    right: 0;
    z-index: 60;
    max-width: calc(100vw - 48px);
    display: flex;
    flex-direction: column;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.55);
    overflow: hidden;
  }
  .pop-left .pop-panel {
    right: auto;
    left: 0;
  }
</style>
