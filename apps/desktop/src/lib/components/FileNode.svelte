<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import FileNode from './FileNode.svelte';
  import type { FileNode as FileNodeData } from '../types';

  // Recursive file tree node with git status dots. Click selects; the
  // editor surface is not built yet.
  interface Props {
    node: FileNodeData;
    depth?: number;
  }

  let { node, depth = 0 }: Props = $props();

  // Uncontrolled open state: shallow dirs start open, user toggles win.
  let openOverride = $state<boolean | null>(null);
  const open = $derived(openOverride ?? depth < 2);
  const selected = $derived(session.selectedPath === node.path);

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
    <button
      class="row file"
      class:selected
      onclick={() => (session.selectedPath = node.path)}
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
  .row.selected {
    background: var(--bg-3);
    color: var(--fg-0);
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
</style>
