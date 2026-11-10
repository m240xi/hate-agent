<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import FileNode from './FileNode.svelte';
  import type { FileNode as FileNodeData } from '../types';

  interface Props {
    node: FileNodeData;
    depth?: number;
  }

  let { node, depth = 0 }: Props = $props();

  let openOverride = $state<boolean | null>(null);
  const open = $derived(openOverride ?? depth < 2);
  const selected = $derived(session.selectedPath === node.path);
  const pinned = $derived(session.isPinned(node.path));

  const STATUS_COLOR: Record<string, string> = {
    M: 'var(--warn)',
    A: 'var(--ok)',
    D: 'var(--err)',
    '?': 'var(--fg-3)',
  };
</script>

<div class="node" style:padding-left="{depth * 14 + 8}px">
  {#if node.kind === 'dir'}
    <button
      class="row dir"
      class:selected
      onclick={() => (openOverride = !open)}
      aria-expanded={open}
    >
      <span class="twisty"><Icon name={open ? 'chevron-down' : 'chevron-right'} size={11} /></span>
      <Icon name="folder" size={13} />
      <span class="name">{node.name}</span>
    </button>
    {#if open}
      {#each node.children ?? [] as child (child.path)}
        <FileNode node={child} depth={depth + 1} />
      {/each}
    {/if}
  {:else}
    <div class="leaf">
      <button
        class="row file"
        class:selected
        class:pinned
        onclick={() => session.openFile(node.path)}
        title={node.path}
      >
        <span class="twisty"></span>
        <Icon name="file" size={13} />
        <span class="name">{node.name}</span>
        {#if node.status}
          <span
            class="dot"
            style:background={STATUS_COLOR[node.status] ?? 'var(--fg-3)'}
            title={node.status === 'M' ? 'modified' : node.status === 'A' ? 'added' : node.status === 'D' ? 'deleted' : 'untracked'}
          ></span>
        {/if}
      </button>
      <button
        class="pin"
        class:on={pinned}
        onclick={() => session.togglePin(node.path)}
        aria-pressed={pinned}
        aria-label={pinned ? `Detach ${node.name}` : `Attach ${node.name} to the next message`}
        title={pinned ? 'Attached to next message — click to remove' : 'Attach to next message'}
      >
        <Icon name={pinned ? 'check' : 'plus'} size={11} />
      </button>
    </div>
  {/if}
</div>

<style>
  .node {
    user-select: none;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    height: 24px;
    padding-right: 8px;
    border-radius: var(--radius);
    color: var(--fg-1);
    text-align: left;
    white-space: nowrap;
  }
  .row:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .row.file {
    padding-right: 26px;
  }
  .row.selected {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .row.pinned .name {
    color: var(--accent);
  }
  .twisty {
    display: inline-flex;
    width: 14px;
    justify-content: center;
    color: var(--fg-3);
    flex: none;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    margin-left: auto;
    flex: none;
  }
  .leaf {
    position: relative;
    display: flex;
  }
  .pin {
    position: absolute;
    right: 4px;
    top: 3px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: var(--radius);
    color: var(--fg-3);
    opacity: 0;
    transition: opacity var(--motion);
  }
  .leaf:hover .pin,
  .pin:focus-visible {
    opacity: 1;
  }
  .pin:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .pin.on {
    opacity: 1;
    color: var(--accent);
  }
</style>
