<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';
  import Pane from './Pane.svelte';
  import FileTree from './FileTree.svelte';
  import TranscriptBlock from './TranscriptBlock.svelte';
  import Composer from './Composer.svelte';
  import PermissionBar from './PermissionBar.svelte';
  import Inspector from './Inspector.svelte';
  import EmptyState from './EmptyState.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import FilePicker from './FilePicker.svelte';
  import type { PermissionRequest, TranscriptBlock as Block } from '../types';

  type Theme = 'dark' | 'light' | 'system';
  const THEMES: Theme[] = ['dark', 'light', 'system'];
  let theme = $state<Theme>('dark');

  $effect(() => {
    const root = document.documentElement;
    if (theme === 'system') delete root.dataset.theme;
    else root.dataset.theme = theme;
  });

  const ICON_NAMES = [
    'folder', 'file', 'search', 'chevron-right', 'chevron-down', 'chevron-left',
    'terminal', 'edit', 'alert', 'check', 'x', 'send', 'sliders', 'list',
    'clock', 'diff', 'panel-left', 'panel-right', 'bolt', 'branch',
  ];

  const sampleBlocks: Block[] = [
    { id: 1, kind: 'user', text: 'Fix the off-by-one in pagination.' },
    { id: 2, kind: 'thinking', text: 'The cursor math looks right; check the inclusive bound.' },
    {
      id: 3,
      kind: 'tool',
      call: { id: 'c1', name: 'edit', args: { path: 'src/page.rs', old: 'i <= n', new: 'i < n' } },
      ok: true,
      fullTokens: 1840,
      sentTokens: 31,
      preview: 'patched src/page.rs:+1 -1',
      expanded: true,
    },
    { id: 4, kind: 'text', text: 'Fixed — `i < n` now excludes the sentinel row. Test passes.' },
  ];

  const fakePermission: PermissionRequest = {
    id: 'perm-demo',
    call: { id: 'c9', name: 'bash', args: { command: 'cargo test --workspace' } },
    reason: 'runs a build script',
    detail: 'cargo test --workspace',
  };

  let showPalette = $state(false);
  let showPicker = $state(false);
  let savedPermission: PermissionRequest | null = null;

  onMount(() => {
    savedPermission = session.pendingPermission;
    session.pendingPermission = fakePermission;
    return () => {
      session.pendingPermission = savedPermission;
    };
  });
</script>

<svelte:head><title>hate-agent — components</title></svelte:head>

<div class="gallery">
  <header class="g-head">
    <h1>components</h1>
    <div class="theme-switch" role="group" aria-label="Theme">
      {#each THEMES as t}
        <button class="tbtn" class:active={theme === t} onclick={() => (theme = t)}>{t}</button>
      {/each}
    </div>
  </header>

  <section class="g-sec">
    <h2>Icons</h2>
    <div class="icon-grid">
      {#each ICON_NAMES as name}
        <div class="icon-cell"><Icon {name} size={18} /><span>{name}</span></div>
      {/each}
    </div>
  </section>

  <section class="g-sec">
    <h2>Pane (collapsible / resizable)</h2>
    <div class="frame" style:height="220px">
      <Pane side="left" title="Demo" collapsed={false} width={220} onToggle={() => {}} onResize={() => {}}>
        <div class="pane-demo">drag the right edge · click « to collapse</div>
      </Pane>
    </div>
  </section>

  <section class="g-sec">
    <h2>FileTree + symbol search</h2>
    <div class="frame" style:height="260px">
      <FileTree />
    </div>
  </section>

  <section class="g-sec">
    <h2>TranscriptBlock — all kinds</h2>
    <div class="frame transcript-demo">
      {#each sampleBlocks as b}
        <TranscriptBlock block={b} />
      {/each}
    </div>
  </section>

  <section class="g-sec">
    <h2>Composer</h2>
    <div class="frame" style:height="140px">
      <Composer />
    </div>
  </section>

  <section class="g-sec">
    <h2>PermissionBar</h2>
    <div class="frame" style:height="180px">
      <PermissionBar />
    </div>
  </section>

  <section class="g-sec">
    <h2>Inspector</h2>
    <div class="frame" style:height="380px">
      <Inspector />
    </div>
  </section>

  <section class="g-sec">
    <h2>EmptyState</h2>
    <div class="frame" style:height="320px">
      <EmptyState />
    </div>
  </section>

  <section class="g-sec">
    <h2>Overlays</h2>
    <div class="row-btns">
      <button class="btn" onclick={() => (showPalette = true)}>open CommandPalette</button>
      <button class="btn" onclick={() => (showPicker = true)}>open FilePicker</button>
    </div>
  </section>
</div>

{#if showPalette}<CommandPalette onclose={() => (showPalette = false)} />{/if}
{#if showPicker}<FilePicker onclose={() => (showPicker = false)} />{/if}

<style>
  .gallery {
    height: 100vh;
    overflow-y: auto;
    padding: 24px 32px 64px;
    background: var(--bg-0);
    color: var(--fg-0);
  }
  .g-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 24px;
  }
  .g-head h1 {
    font-size: 18px;
    margin: 0;
  }
  .theme-switch {
    display: flex;
    gap: 4px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 2px;
  }
  .tbtn {
    padding: 3px 10px;
    font-size: 11.5px;
    border-radius: 4px;
    color: var(--fg-2);
  }
  .tbtn.active {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .g-sec {
    margin-bottom: 28px;
  }
  .g-sec h2 {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-3);
    margin: 0 0 10px;
  }
  .icon-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(90px, 1fr));
    gap: 6px;
  }
  .icon-cell {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 10px 4px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-1);
    color: var(--fg-1);
  }
  .icon-cell span {
    font-family: var(--font-mono);
    font-size: 9.5px;
    color: var(--fg-3);
  }
  .frame {
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    overflow: hidden;
    display: flex;
    background: var(--bg-1);
  }
  .pane-demo {
    padding: 16px;
    font-size: 12px;
    color: var(--fg-2);
  }
  .transcript-demo {
    flex-direction: column;
    gap: 10px;
    padding: 16px;
    overflow-y: auto;
  }
  .row-btns {
    display: flex;
    gap: 10px;
  }
  .btn {
    padding: 5px 14px;
    font-size: 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-2);
    color: var(--fg-1);
  }
  .btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
</style>
