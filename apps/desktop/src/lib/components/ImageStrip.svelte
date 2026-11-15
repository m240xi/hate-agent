<script lang="ts">
  import Icon from './Icon.svelte';
  import ImagePreview from './ImagePreview.svelte';

  let { sources }: { sources: string[] } = $props();

  const label = $derived(
    sources.length === 1 ? '1 image' : `${sources.length} images`,
  );
</script>

<section class="image-strip rise" aria-label={`Generated ${label}`}>
  <header class="strip-head">
    <Icon name="image" size={11} />
    <span class="strip-title">generated</span>
    <span class="strip-count mono">{label}</span>
  </header>
  <div class="strip-grid">
    {#each sources as source (source)}
      <ImagePreview {source} thumb />
    {/each}
  </div>
</section>

<style>
  .image-strip {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--fg-0) 2.5%, var(--bg-1)),
      var(--bg-1)
    );
  }
  .strip-head {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--fg-3);
  }
  .strip-title {
    font-size: 10px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .strip-count {
    font-size: 10.5px;
    color: var(--fg-3);
  }
  .strip-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 8px;
  }
</style>
