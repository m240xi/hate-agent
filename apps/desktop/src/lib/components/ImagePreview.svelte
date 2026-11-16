<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import Icon from './Icon.svelte';
  import { trapFocus } from '../focus';
  import { isInlineImage } from '../images';

  let { source, thumb = false }: { source: string; thumb?: boolean } = $props();

  let src = $state('');
  let error = $state('');
  let open = $state(false);
  let saving = $state(false);
  let saved = $state('');

  const name = $derived(
    isInlineImage(source) ? 'generated image' : (source.split(/[\\/]/).pop() ?? 'image'),
  );

  $effect(() => {
    const current = source;
    let active = true;
    src = '';
    error = '';
    saved = '';
    if (isInlineImage(current)) src = current;
    else {
      void invoke<string>('image_read', { path: current }).then(
        (value) => { if (active) src = value; },
        (reason) => { if (active) error = String(reason); },
      );
    }
    return () => { active = false; };
  });

  $effect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        open = false;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  async function save(): Promise<void> {
    if (saving) return;
    saving = true;
    saved = '';
    try {
      const path = await invoke<string | null>('image_save', { source });
      saved = path ? `Saved to ${path}` : '';
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function reveal(): Promise<void> {
    try {
      await invoke('image_reveal', { path: source });
    } catch (e) {
      error = String(e);
    }
  }
</script>

<button
  class="thumb"
  class:mini={thumb}
  onclick={() => (open = true)}
  aria-label={`Enlarge ${name}`}
  title={name}
>
  {#if src}
    <img
      {src}
      alt={thumb ? name : 'AI-generated result'}
      data-ctx="image"
      onerror={() => (error = 'The image could not be decoded.')}
    />
  {:else if error}
    <span class="fail" title={error}>preview unavailable</span>
  {:else}
    <span class="loading mono">…</span>
  {/if}
</button>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="lightbox"
    role="dialog"
    aria-modal="true"
    aria-label={name}
    tabindex="-1"
    use:trapFocus
    onclick={() => (open = false)}
  >
    <div class="panel" onclick={(e) => e.stopPropagation()}>
      <div class="bar">
        <span class="name mono" title={source}>{name}</span>
        <button class="btn sm" onclick={save} disabled={saving}>
          <Icon name="download" size={11} />
          {saving ? 'choosing…' : 'Download'}
        </button>
        {#if !isInlineImage(source)}
          <button class="btn sm ghost" onclick={reveal} title="Show in file explorer">
            <Icon name="folder" size={11} />
            Show in folder
          </button>
        {/if}
        <button class="btn sm ghost" onclick={() => (open = false)} aria-label="Close preview">
          <Icon name="x" size={11} />
        </button>
      </div>
      {#if src}
        <img class="full" {src} alt="AI-generated result" data-ctx="image" />
      {:else}
        <p class="err">{error || 'Loading…'}</p>
      {/if}
      {#if saved}<p class="note">{saved}</p>{:else if error && src}<p class="note err">{error}</p>{/if}
    </div>
  </div>
{/if}

<style>
  .thumb {
    display: block;
    width: 100%;
    padding: 0;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-0);
    cursor: zoom-in;
    transition:
      border-color var(--motion),
      transform var(--motion);
  }
  .thumb:hover {
    border-color: var(--border-strong);
    transform: translateY(-1px);
  }
  .thumb img {
    display: block;
    width: 100%;
    height: 100%;
    max-height: 220px;
    object-fit: cover;
  }
  .thumb.mini img {
    max-height: 96px;
  }
  .fail,
  .loading {
    display: block;
    padding: 18px 10px;
    font-size: 10.5px;
    color: var(--fg-3);
  }

  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    padding: 32px;
    background: color-mix(in srgb, var(--bg-0) 82%, transparent);
    backdrop-filter: blur(3px);
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: min(1100px, 100%);
    max-height: 100%;
    padding: 10px;
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--el-3);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .name {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .full {
    display: block;
    max-width: 100%;
    max-height: calc(100vh - 160px);
    object-fit: contain;
    border-radius: var(--radius);
  }
  .err {
    margin: 0;
    padding: 12px;
    font-size: 12px;
    color: var(--err);
  }
  .note {
    margin: 0;
    font-size: 11px;
    color: var(--fg-3);
    overflow-wrap: anywhere;
  }
  .note.err {
    color: var(--err);
  }
</style>
