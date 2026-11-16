<script lang="ts">
  import { getProviderLogo } from '../provider-logos';

  interface Props {
    provider?: string | null;
    label?: string | null;
    size?: number;
    fallbackFontSize?: number;
  }

  let { provider = null, label = null, size = 14, fallbackFontSize }: Props = $props();

  const paths = $derived(getProviderLogo(provider));
  const initial = $derived.by(() => {
    const src = (label || provider || '?').trim();
    return (src.charAt(0) || '?').toUpperCase();
  });
  const letterSize = $derived(fallbackFontSize ?? Math.max(12, Math.round(size * 1.15)));
</script>

{#if paths}
  <svg
    class="provider-logo"
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="currentColor"
    fill-rule="evenodd"
    aria-hidden="true"
  >
    {#each paths as p}
      <path
        d={p.d}
        clip-rule={p.clipRule}
        fill-opacity={p.fillOpacity}
      />
    {/each}
  </svg>
{:else}
  <span
    class="provider-fallback"
    style:font-size="{letterSize}px"
    aria-hidden="true"
  >
    {initial}
  </span>
{/if}

<style>
  .provider-logo {
    display: inline-block;
    flex: none;
    vertical-align: middle;
  }
  .provider-fallback {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    font-family: 'm5x7', var(--font-mono);
    line-height: 1;
  }
</style>
