<script lang="ts">
  import catUrl from '../../assets/sprites/gattino.png?url';

  interface Props {
    size?: number;
    zzz?: boolean;
  }

  let { size = 64, zzz = true }: Props = $props();
</script>

<span class="cat" style:--s="{size}px" aria-hidden="true">
  <span class="sprite" style:background-image="url({catUrl})"></span>
  {#if zzz}
    <span class="z z1">z</span>
    <span class="z z2">z</span>
  {/if}
</span>

<style>
  .cat {
    position: relative;
    display: inline-block;
    width: var(--s);
    height: var(--s);
    flex: none;
  }
  .sprite {
    display: block;
    width: 100%;
    height: 100%;
    background-repeat: no-repeat;
    background-size: calc(var(--s) * 2) var(--s);
    image-rendering: pixelated;
    animation: frames 1.8s steps(2, jump-none) infinite;
  }
  @keyframes frames {
    from {
      background-position: 0 0;
    }
    to {
      background-position: calc(var(--s) * -1) 0;
    }
  }
  .z {
    position: absolute;
    top: 4%;
    right: 12%;
    font-family: 'm5x7', var(--font-mono);
    font-size: calc(var(--s) * 0.22);
    line-height: 1;
    color: var(--fg-2);
    opacity: 0;
    animation: float 3.2s ease-out infinite;
  }
  .z2 {
    animation-delay: 1.6s;
  }
  /* The blink and the zzz are character animation, not decorative motion, so
     they ride on `data-motion` alone and survive "system + OS reduced"; only an
     explicit "Reduced" stops them (see app.css). */
  @keyframes float {
    0% {
      transform: translate(0, 0);
      opacity: 0;
    }
    20% {
      opacity: 0.9;
    }
    100% {
      transform: translate(calc(var(--s) * 0.16), calc(var(--s) * -0.34));
      opacity: 0;
    }
  }
</style>
