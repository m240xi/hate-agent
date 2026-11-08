<script lang="ts">
  // NightScene — the landing's moving background: a procedural pixel-art
  // night landscape drawn into a low-res canvas and upscaled with
  // `image-rendering: pixelated`, so it reads as real pixel art, not a
  // blurred gradient. ~8fps like GG Coder's idle loop; a single still
  // frame under prefers-reduced-motion.
  //
  // Palette is the app's mono chrome plus ONE warm accent — the
  // campfire and its embers — which is the whole 3am-club signature.
  import { onMount } from 'svelte';

  let canvas = $state<HTMLCanvasElement | null>(null);

  const L_H = 230; // logical pixel height; width follows the aspect ratio
  const FPS_MS = 125;

  // Deterministic layout per resize so the scene doesn't reshuffle.
  function mulberry(seed: number): () => number {
    let s = seed >>> 0;
    return () => {
      s = (s + 0x6d2b79f5) >>> 0;
      let t = s;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  interface Star {
    x: number;
    y: number;
    b: number;
    ph: number;
    sp: number;
    big: boolean;
  }
  interface Fly {
    x0: number;
    y0: number;
    rx: number;
    ry: number;
    ph: number;
    sp: number;
  }

  interface Scene {
    w: number;
    horizon: number;
    stars: Star[];
    ridge: number[];
    trees: number[];
    grass: number[];
    flies: Fly[];
    fireX: number;
    fireY: number;
  }

  function build(w: number): Scene {
    const rnd = mulberry(0x3a7);
    const horizon = Math.round(L_H * 0.74);

    const stars: Star[] = [];
    for (let i = 0; i < Math.round(w * 0.32); i++) {
      stars.push({
        x: Math.floor(rnd() * w),
        y: Math.floor(rnd() * (horizon - 30)),
        b: 0.35 + rnd() * 0.65,
        ph: rnd() * Math.PI * 2,
        sp: 0.4 + rnd() * 1.4,
        big: rnd() < 0.06,
      });
    }

    // Far ridge: two slow sines plus jitter — soft rolling silhouette.
    const ridge: number[] = [];
    const p1 = rnd() * 9;
    const p2 = rnd() * 9;
    for (let x = 0; x < w; x++) {
      const n =
        Math.sin(x / 47 + p1) * 6 + Math.sin(x / 23 + p2) * 3 + (rnd() - 0.5) * 3;
      ridge.push(Math.round(horizon - 16 - n));
    }

    // Pine treeline: random-walk base with sharp spike peaks.
    const trees: number[] = [];
    let h = 20;
    for (let x = 0; x < w; x++) {
      h += (rnd() - 0.5) * 2.4;
      h = Math.max(10, Math.min(34, h));
      let col = Math.round(horizon - 4 - h);
      // occasional tapered peak — a tall pine crown
      if (rnd() < 0.05) {
        const peak = 6 + Math.floor(rnd() * 8);
        const spread = 2 + Math.floor(rnd() * 2);
        for (let dx = -spread; dx <= spread; dx++) {
          const xi = x + dx;
          if (xi >= 0 && xi < w) {
            const lift = peak - Math.abs(dx) * 3;
            const ny = Math.round(horizon - 4 - h - Math.max(0, lift));
            if (trees[xi] === undefined || ny < trees[xi]) {
              trees[xi] = ny;
            }
          }
        }
      }
      if (trees[x] === undefined) trees[x] = col;
    }
    for (let x = 0; x < w; x++) {
      trees[x] ??= Math.round(horizon - 4 - h);
    }

    const grass: number[] = [];
    for (let x = 0; x < w; x++) {
      grass.push(rnd() < 0.3 ? Math.floor(rnd() * (L_H - horizon - 6)) : -1);
    }

    const flies: Fly[] = [];
    for (let i = 0; i < 6; i++) {
      flies.push({
        x0: w * 0.15 + rnd() * w * 0.7,
        y0: horizon + 4 + rnd() * (L_H - horizon - 16),
        rx: 6 + rnd() * 14,
        ry: 2 + rnd() * 5,
        ph: rnd() * Math.PI * 2,
        sp: 0.25 + rnd() * 0.5,
      });
    }

    const fireX = Math.round(w * 0.46);
    const fireY = horizon + 14;
    return { w, horizon, stars, ridge, trees, grass, flies, fireX, fireY };
  }

  function draw(ctx: CanvasRenderingContext2D, s: Scene, t: number): void {
    const { w } = s;
    ctx.imageSmoothingEnabled = false;

    // Sky — black with a barely-there warm lift at the horizon.
    ctx.fillStyle = '#000000';
    ctx.fillRect(0, 0, w, L_H);
    const sky = ctx.createLinearGradient(0, s.horizon - 26, 0, s.horizon + 2);
    sky.addColorStop(0, 'rgba(20, 12, 8, 0)');
    sky.addColorStop(1, 'rgba(34, 20, 10, 0.35)');
    ctx.fillStyle = sky;
    ctx.fillRect(0, s.horizon - 26, w, 28);

    // Stars — twinkle at different speeds.
    for (const st of s.stars) {
      const a = st.b * (0.55 + 0.45 * Math.sin(t * 0.001 * st.sp + st.ph));
      ctx.fillStyle = `rgba(232, 230, 223, ${a.toFixed(3)})`;
      ctx.fillRect(st.x, st.y, 1, 1);
      if (st.big) {
        ctx.fillRect(st.x - 1, st.y, 1, 1);
        ctx.fillRect(st.x + 1, st.y, 1, 1);
        ctx.fillRect(st.x, st.y - 1, 1, 1);
        ctx.fillRect(st.x, st.y + 1, 1, 1);
      }
    }

    // Moon — pixel disc, upper right, two darker craters.
    const mx = Math.round(w * 0.79);
    const my = 34;
    const mr = 8;
    ctx.fillStyle = '#efede6';
    for (let dy = -mr; dy <= mr; dy++) {
      for (let dx = -mr; dx <= mr; dx++) {
        if (dx * dx + dy * dy <= mr * mr) ctx.fillRect(mx + dx, my + dy, 1, 1);
      }
    }
    ctx.fillStyle = '#c8c5bc';
    ctx.fillRect(mx - 2, my - 3, 3, 2);
    ctx.fillRect(mx + 2, my + 1, 2, 2);
    ctx.fillRect(mx - 1, my + 3, 1, 1);

    // Far ridge silhouette.
    ctx.fillStyle = '#0a0a0e';
    for (let x = 0; x < w; x++) {
      const y = s.ridge[x] ?? s.horizon;
      ctx.fillRect(x, y, 1, s.horizon + 2 - y);
    }

    // Pine treeline silhouette — darker, spikier.
    ctx.fillStyle = '#050505';
    for (let x = 0; x < w; x++) {
      const y = s.trees[x] ?? s.horizon;
      ctx.fillRect(x, y, 1, s.horizon + 3 - y);
    }
    // A few front tufts one shade up so the line reads as depth.
    ctx.fillStyle = '#0c0c0f';
    for (let x = 0; x < w; x += 13) {
      const y = s.trees[Math.min(w - 1, x + 3)] ?? s.horizon;
      ctx.fillRect(x + 3, y - 2, 1, 3);
    }

    // Meadow — near-black ground with sparse grass ticks.
    ctx.fillStyle = '#030403';
    ctx.fillRect(0, s.horizon + 2, w, L_H - s.horizon - 2);
    ctx.fillStyle = '#0d120c';
    for (let x = 0; x < w; x++) {
      const g = s.grass[x] ?? -1;
      if (g >= 0) ctx.fillRect(x, s.horizon + 4 + g, 1, 1);
    }

    // Campfire — the one warm accent. Soft glow, logs, flickering core.
    const { fireX: fx, fireY: fy } = s;
    const flick = 0.75 + 0.25 * Math.sin(t * 0.012) + 0.1 * Math.sin(t * 0.031);
    const glow = ctx.createRadialGradient(fx, fy, 2, fx, fy, 30);
    glow.addColorStop(0, `rgba(255, 168, 74, ${(0.16 * flick).toFixed(3)})`);
    glow.addColorStop(1, 'rgba(255, 168, 74, 0)');
    ctx.fillStyle = glow;
    ctx.fillRect(fx - 30, fy - 30, 60, 60);
    // ground glow
    ctx.fillStyle = `rgba(224, 122, 45, ${(0.1 * flick).toFixed(3)})`;
    ctx.fillRect(fx - 9, fy + 3, 18, 2);

    ctx.fillStyle = '#3a2415'; // logs
    ctx.fillRect(fx - 4, fy + 1, 9, 2);
    ctx.fillRect(fx - 3, fy + 3, 7, 1);

    const core = 3 + Math.floor(flick * 2.4);
    ctx.fillStyle = '#ffd57a';
    ctx.fillRect(fx - 1, fy - core, 3, core);
    ctx.fillStyle = '#ff9d45';
    ctx.fillRect(fx - 2, fy - Math.floor(core * 0.6), 5, Math.floor(core * 0.6));
    ctx.fillStyle = '#e0542f';
    ctx.fillRect(fx - 1, fy - 1, 3, 2);

    // Rising sparks — three embers on offset loops.
    ctx.fillStyle = '#ffb03a';
    for (let i = 0; i < 3; i++) {
      const age = (t * 0.02 + i * 37) % 40;
      const sx = fx + Math.round(Math.sin(t * 0.001 + i * 2.1) * (2 + i));
      const sy = fy - core - Math.floor(age);
      if (sy > 0 && age < 30) ctx.fillRect(sx, sy, 1, 1);
    }

    // Fireflies — slow elliptical drift, blinking in and out.
    for (const f of s.flies) {
      const blink = Math.sin(t * 0.0016 * f.sp * 6 + f.ph * 3);
      if (blink < -0.2) continue;
      const x = Math.round(f.x0 + Math.cos(t * 0.0006 * f.sp + f.ph) * f.rx);
      const y = Math.round(f.y0 + Math.sin(t * 0.0009 * f.sp + f.ph * 1.7) * f.ry);
      ctx.fillStyle = `rgba(232, 200, 106, ${(0.4 + 0.6 * blink).toFixed(3)})`;
      ctx.fillRect(x, y, 1, 1);
    }
  }

  onMount(() => {
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    let scene = build(300);
    let timer: ReturnType<typeof setInterval> | null = null;

    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    const resize = () => {
      if (!canvas) return;
      const rect = canvas.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      canvas.height = L_H;
      canvas.width = Math.max(200, Math.round((rect.width / rect.height) * L_H));
      scene = build(canvas.width);
      draw(ctx, scene, performance.now());
    };

    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    resize();

    if (!reduced) {
      timer = setInterval(() => {
        if (!document.hidden) draw(ctx, scene, performance.now());
      }, FPS_MS);
    }

    return () => {
      ro.disconnect();
      if (timer) clearInterval(timer);
    };
  });
</script>

<canvas bind:this={canvas} class="scene" aria-hidden="true"></canvas>

<style>
  .scene {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    image-rendering: pixelated;
  }
</style>
