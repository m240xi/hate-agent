<script lang="ts">
  import { onMount } from 'svelte';
  import catUrl from '../../assets/sprites/gattino.png?url';

  let canvas = $state<HTMLCanvasElement | null>(null);

  const L_H = 230;
  const FPS_MS = 125;

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

  const SHEET_FILES = {
    cat: catUrl,
  } as const;
  type SheetId = keyof typeof SHEET_FILES;

  const SHEETS: Record<SheetId, { fw: number; fh: number; cols: number; count: number; oy: number }> = {
    cat: { fw: 64, fh: 64, cols: 2, count: 2, oy: 0 },
  };

  interface Pal {
    sheet: SheetId;
    x: number;
    base: number;
    fps: number;
    flip?: boolean;
    scale: number;
    smooth?: boolean;
    zzz?: boolean;
  }

  const images = new Map<SheetId, HTMLImageElement>();

  interface Scene {
    w: number;
    horizon: number;
    stars: Star[];
    ridge: number[];
    trees: number[];
    grass: number[];
    flies: Fly[];
    pals: Pal[];
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

    const ridge: number[] = [];
    const p1 = rnd() * 9;
    const p2 = rnd() * 9;
    for (let x = 0; x < w; x++) {
      const n =
        Math.sin(x / 47 + p1) * 6 + Math.sin(x / 23 + p2) * 3 + (rnd() - 0.5) * 3;
      ridge.push(Math.round(horizon - 16 - n));
    }

    const trees: number[] = [];
    let h = 20;
    for (let x = 0; x < w; x++) {
      h += (rnd() - 0.5) * 2.4;
      h = Math.max(10, Math.min(34, h));
      let col = Math.round(horizon - 4 - h);
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

    const pals: Pal[] = [
      { sheet: 'cat', x: Math.round(w * 0.5), base: L_H - 7, fps: 1.1, scale: 0.75, zzz: true },
    ];
    return { w, horizon, stars, ridge, trees, grass, flies, pals };
  }

  function draw(ctx: CanvasRenderingContext2D, s: Scene, t: number): void {
    const { w } = s;
    ctx.imageSmoothingEnabled = false;

    ctx.fillStyle = '#000000';
    ctx.fillRect(0, 0, w, L_H);
    const sky = ctx.createLinearGradient(0, s.horizon - 26, 0, s.horizon + 2);
    sky.addColorStop(0, 'rgba(20, 12, 8, 0)');
    sky.addColorStop(1, 'rgba(34, 20, 10, 0.35)');
    ctx.fillStyle = sky;
    ctx.fillRect(0, s.horizon - 26, w, 28);

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

    ctx.fillStyle = '#0a0a0e';
    for (let x = 0; x < w; x++) {
      const y = s.ridge[x] ?? s.horizon;
      ctx.fillRect(x, y, 1, s.horizon + 2 - y);
    }

    ctx.fillStyle = '#050505';
    for (let x = 0; x < w; x++) {
      const y = s.trees[x] ?? s.horizon;
      ctx.fillRect(x, y, 1, s.horizon + 3 - y);
    }
    ctx.fillStyle = '#0c0c0f';
    for (let x = 0; x < w; x += 13) {
      const y = s.trees[Math.min(w - 1, x + 3)] ?? s.horizon;
      ctx.fillRect(x + 3, y - 2, 1, 3);
    }

    ctx.fillStyle = '#030403';
    ctx.fillRect(0, s.horizon + 2, w, L_H - s.horizon - 2);
    ctx.fillStyle = '#0d120c';
    for (let x = 0; x < w; x++) {
      const g = s.grass[x] ?? -1;
      if (g >= 0) ctx.fillRect(x, s.horizon + 4 + g, 1, 1);
    }

    for (const p of s.pals) {
      const img = images.get(p.sheet);
      if (!img) continue;
      const sheet = SHEETS[p.sheet];
      const fi = Math.floor(t * 0.001 * p.fps) % sheet.count;
      const sx = (fi % sheet.cols) * sheet.fw;
      const sy = sheet.oy + Math.floor(fi / sheet.cols) * sheet.fh;
      const dw = sheet.fw * p.scale;
      const dh = sheet.fh * p.scale;
      const pool = ctx.createRadialGradient(p.x, p.base - dh * 0.5, 1, p.x, p.base - dh * 0.5, 24);
      pool.addColorStop(0, 'rgba(255,150,60,0.10)');
      pool.addColorStop(1, 'rgba(255,150,60,0)');
      ctx.fillStyle = pool;
      ctx.fillRect(p.x - 24, p.base - dh - 14, 48, dh + 28);
      const bob = p.zzz ? Math.sin(t * 0.0016) * 0.7 : 0;
      const dy = Math.round(p.base - dh + bob);
      const dx = Math.round(p.x - dw / 2);
      if (p.smooth) ctx.imageSmoothingEnabled = true;
      if (p.flip) {
        ctx.save();
        ctx.translate(Math.round(p.x), 0);
        ctx.scale(-1, 1);
        ctx.drawImage(img, sx, sy, sheet.fw, sheet.fh, Math.round(-dw / 2), dy, dw, dh);
        ctx.restore();
      } else {
        ctx.drawImage(img, sx, sy, sheet.fw, sheet.fh, dx, dy, dw, dh);
      }
      if (p.smooth) ctx.imageSmoothingEnabled = false;

      if (p.zzz) {
        const zx = p.x + dw * 0.3;
        const zy = p.base - dh - 4;
        for (let i = 0; i < 3; i++) {
          const cyc = (t * 0.00035 + i * 0.37) % 1;
          const a = Math.min(1, (1 - cyc) * 1.6) * 0.7;
          if (a <= 0.03) continue;
          const px = Math.round(zx + i * 2.5 + Math.sin(cyc * 6 + i * 2) * 1.5);
          const py = Math.round(zy - cyc * 24);
          ctx.fillStyle = `rgba(239,237,230,${a.toFixed(3)})`;
          const Z = ['###', '..#', '.#.', '#..', '###'];
          for (let r = 0; r < Z.length; r++) {
            const row = Z[r] ?? '';
            for (let c = 0; c < row.length; c++) {
              if (row[c] === '#') ctx.fillRect(px + c, py + r, 1, 1);
            }
          }
        }
      }
    }

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

    void Promise.all(
      (Object.keys(SHEET_FILES) as SheetId[]).map(
        (id) =>
          new Promise<void>((resolve) => {
            const img = new Image();
            img.onload = () => {
              images.set(id, img);
              resolve();
            };
            img.onerror = () => resolve();
            img.src = SHEET_FILES[id];
          }),
      ),
    ).then(() => draw(ctx, scene, performance.now()));

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
