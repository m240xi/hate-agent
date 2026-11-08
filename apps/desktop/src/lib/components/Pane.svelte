<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  // Collapsible, resizable side pane. Width drags on the inner edge;
  // collapsed state leaves a slim rail with an expand affordance.
  interface Props {
    side: 'left' | 'right';
    title: string;
    collapsed: boolean;
    width: number;
    min?: number;
    max?: number;
    onToggle: () => void;
    onResize: (w: number) => void;
    children: Snippet;
  }

  let { side, title, collapsed, width, min = 180, max = 560, onToggle, onResize, children }: Props =
    $props();

  let dragging = $state(false);

  function startDrag(e: PointerEvent): void {
    e.preventDefault();
    dragging = true;
    const startX = e.clientX;
    const startW = width;
    const dir = side === 'left' ? 1 : -1;

    function move(ev: PointerEvent): void {
      const w = Math.min(max, Math.max(min, startW + (ev.clientX - startX) * dir));
      onResize(w);
    }
    function up(): void {
      dragging = false;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    }
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }
</script>

{#if collapsed}
  <aside class="rail {side}" aria-label={title}>
    <button class="rail-btn" onclick={onToggle} aria-label={`Expand ${title}`} title={title}>
      <Icon name={side === 'left' ? 'chevron-right' : 'chevron-left'} size={13} />
    </button>
    <span class="rail-title" style:writing-mode="vertical-rl">{title}</span>
  </aside>
{:else}
  <aside
    class="pane {side}"
    style:width="{width}px"
    aria-label={title}
  >
    <header class="pane-head">
      <span class="pane-title">{title}</span>
      <button
        class="icon-btn"
        onclick={onToggle}
        aria-label={`Collapse ${title}`}
        title={`Collapse ${title}`}
      >
        <Icon name={side === 'left' ? 'chevron-left' : 'chevron-right'} size={13} />
      </button>
    </header>
    <div class="pane-body">
      {@render children()}
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="resizer"
      class:active={dragging}
      role="separator"
      aria-orientation="vertical"
      aria-label={`Resize ${title}`}
      onpointerdown={startDrag}
    ></div>
  </aside>
{/if}

<style>
  .pane {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    background: var(--bg-1);
    border-right: 1px solid var(--border);
  }
  .pane.right {
    border-right: none;
    border-left: 1px solid var(--border);
  }
  .pane-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 30px;
    padding: 0 8px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .pane-title {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-2);
  }
  .pane-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .resizer {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 5px;
    cursor: col-resize;
    z-index: 5;
  }
  .pane.left .resizer {
    right: -2px;
  }
  .pane.right .resizer {
    left: -2px;
  }
  .resizer:hover,
  .resizer.active {
    background: var(--accent-dim);
    opacity: 0.5;
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: var(--radius);
    color: var(--fg-2);
  }
  .icon-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .rail {
    width: 28px;
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 8px 0;
    background: var(--bg-1);
    border-right: 1px solid var(--border);
  }
  .rail.right {
    border-right: none;
    border-left: 1px solid var(--border);
  }
  .rail-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: var(--radius);
    color: var(--fg-2);
  }
  .rail-btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .rail-title {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
</style>
