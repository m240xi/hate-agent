<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    trigger: Snippet;
    panel: Snippet<[{ close: () => void }]>;
    onOpenChange?: (open: boolean) => void;
    align?: 'left' | 'right';
    width?: string;
    class?: string;
  }

  let {
    trigger,
    panel,
    onOpenChange,
    align = 'right',
    width,
    class: className = '',
  }: Props = $props();

  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);
  let down = $state(false);
  let room = $state(480);

  function measure(): void {
    if (!root) return;
    const r = root.getBoundingClientRect();
    const above = r.top - 16;
    const below = window.innerHeight - r.bottom - 16;
    down = below > above;
    room = Math.max(160, Math.floor((down ? below : above) - 8));
  }

  function setOpen(v: boolean): void {
    if (open === v) return;
    if (v) measure();
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

<svelte:window
  onclick={onWindowClick}
  onkeydowncapture={onKeydown}
  onresize={() => open && measure()}
/>

<div class="pop {className}" class:pop-left={align === 'left'} bind:this={root}>
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
    <div
      class="pop-panel"
      class:down
      role="dialog"
      style:width={width}
      style:--pop-max="{room}px"
    >
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
    max-height: var(--pop-max, 70vh);
    display: flex;
    flex-direction: column;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--el-3);
    overflow: hidden;
  }
  .pop-panel.down {
    bottom: auto;
    top: calc(100% + 8px);
  }
  .pop-left .pop-panel {
    right: auto;
    left: 0;
  }
</style>
