<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import FileTree from './FileTree.svelte';
  import Inspector from './Inspector.svelte';
  import { slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { ms } from '../motion';

  interface Props {
    files: boolean;
  }

  let { files }: Props = $props();

  const tab = $derived(files ? session.panelTab : 'activity');
  let dragging = $state(false);

  function startDrag(e: PointerEvent): void {
    e.preventDefault();
    dragging = true;
    const startX = e.clientX;
    const startW = session.rightWidth;
    const max = Math.max(
      240,
      Math.min(560, window.innerWidth - (session.sidebarCollapsed ? 0 : 260) - 420),
    );
    const move = (ev: PointerEvent) => {
      session.rightWidth = Math.min(max, Math.max(240, startW - (ev.clientX - startX)));
    };
    const up = () => {
      dragging = false;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }
</script>

<aside
  class="panel glass-pane"
  style:width="{session.rightWidth}px"
  style:--panel-w="{session.rightWidth - 1}px"
  aria-label="Side panel"
  transition:slide={{ axis: 'x', duration: ms(200), easing: cubicOut }}
>
  <header class="head">
    {#if files}
      <div class="tabs" role="tablist">
        <button
          role="tab"
          class:on={tab === 'files'}
          aria-selected={tab === 'files'}
          onclick={() => (session.panelTab = 'files')}
        >
          <Icon name="folder" size={13} /> Files
        </button>
        <button
          role="tab"
          class:on={tab === 'activity'}
          aria-selected={tab === 'activity'}
          onclick={() => (session.panelTab = 'activity')}
        >
          <Icon name="activity" size={13} /> Activity
        </button>
      </div>
    {:else}
      <span class="title">Activity</span>
    {/if}
    <span class="grow"></span>
    <button
      class="close"
      onclick={() => session.setPanel(false)}
      aria-label="Close panel"
      title="Close panel"
    >
      <Icon name="x" size={13} />
    </button>
  </header>
  <div class="body">
    {#if tab === 'files'}
      <FileTree />
    {:else}
      <Inspector />
    {/if}
  </div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="resizer"
    class:active={dragging}
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize panel"
    onpointerdown={startDrag}
  ></div>
</aside>

<style>
  .panel {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg-1);
    border-left: 1px solid var(--border);
    overflow: hidden;
  }
  .head,
  .body {
    min-width: var(--panel-w);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 44px;
    padding: 0 8px 0 10px;
    flex: none;
    border-bottom: 1px solid var(--border);
  }
  .tabs {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    border-radius: 10px;
    background: var(--bg-2);
  }
  .tabs button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border-radius: 7px;
    font-size: 12px;
    color: var(--fg-2);
  }
  .tabs button:hover {
    color: var(--fg-0);
  }
  .tabs button.on {
    background: var(--bg-0);
    color: var(--fg-0);
    box-shadow: var(--el-1);
  }
  .title {
    padding-left: 4px;
    font-size: 13px;
    font-weight: 600;
    color: var(--fg-1);
  }
  .grow {
    flex: 1;
  }
  .close {
    display: inline-flex;
    padding: 6px;
    border-radius: 7px;
    color: var(--fg-3);
  }
  .close:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .body {
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
    left: 0;
    width: 5px;
    cursor: col-resize;
    z-index: 5;
  }
  .resizer:hover,
  .resizer.active {
    background: var(--accent-dim);
    opacity: 0.5;
  }
</style>
