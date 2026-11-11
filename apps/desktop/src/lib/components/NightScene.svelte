<script lang="ts">
  import { onMount } from 'svelte';
  import catUrl from '../../assets/sprites/gattino.png?url';
  import skyUrl from '../../assets/scene/sky.png?url';
  import skyMoonlessUrl from '../../assets/scene/sky-moonless.png?url';
  import mountainsFarUrl from '../../assets/scene/mountains-far.png?url';
  import mountainsUrl from '../../assets/scene/mountains.png?url';
  import treesUrl from '../../assets/scene/trees.png?url';
  import treesFrontUrl from '../../assets/scene/trees-front.png?url';

  let canvas = $state<HTMLCanvasElement | null>(null);

  // Landscape: "Mountain at Dusk" by ansimuz (CC0), re-graded to night.
  // See src/assets/scene/README.md.
  const L_H = 220;
  const FPS_MS = 125;
  const MOON = { cx: 135, cy: 39, tile: 272 };
  const MEADOW_Y = 157;
  const OPEN_SKY = new Set(['9,11,24', '18,23,51']);

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

  const LAYER_FILES = {
    sky: skyUrl,
    skyMoonless: skyMoonlessUrl,
    mountainsFar: mountainsFarUrl,
    mountains: mountainsUrl,
    trees: treesUrl,
    treesFront: treesFrontUrl,
  } as const;
  type LayerId = keyof typeof LAYER_FILES;
  const layers = new Map<LayerId, HTMLImageElement>();

  interface Scene {
    w: number;
    moonX: number;
    stars: Star[];
    flies: Fly[];
    pals: Pal[];
    /** Sky with moon and glow; stars go on top of this. */
    back: HTMLCanvasElement | null;
    /** Mountains, trees and meadow over transparent sky. */
    front: HTMLCanvasElement | null;
  }

  function buffer(w: number): [HTMLCanvasElement, CanvasRenderingContext2D] | null {
    const c = document.createElement('canvas');
    c.width = w;
    c.height = L_H;
    const ctx = c.getContext('2d', { willReadFrequently: true });
    if (!ctx) return null;
    ctx.imageSmoothingEnabled = false;
    return [c, ctx];
  }

  function tile(ctx: CanvasRenderingContext2D, img: HTMLImageElement, origin: number, w: number): void {
    const start = origin - Math.ceil(origin / img.width) * img.width;
    for (let x = start; x < w; x += img.width) ctx.drawImage(img, x, 0);
  }

  function paintBack(w: number, moonX: number): HTMLCanvasElement | null {
    const sky = layers.get('sky');
    const plain = layers.get('skyMoonless');
    const made = buffer(w);
    if (!sky || !plain || !made) return null;
    const [c, ctx] = made;
    ctx.fillStyle = '#090b18';
    ctx.fillRect(0, 0, w, L_H);
    // The sky tile repeats on wide windows; only one copy keeps its moon.
    const origin = moonX - MOON.cx;
    tile(ctx, plain, origin, w);
    ctx.drawImage(sky, origin, 0);
    const glow = ctx.createRadialGradient(moonX, MOON.cy, 20, moonX, MOON.cy, 64);
    glow.addColorStop(0, 'rgba(170, 185, 255, 0.10)');
    glow.addColorStop(1, 'rgba(170, 185, 255, 0)');
    ctx.fillStyle = glow;
    ctx.fillRect(moonX - 64, 0, 128, MOON.cy + 64);
    return c;
  }

  function paintFront(w: number, rnd: () => number): HTMLCanvasElement | null {
    const made = buffer(w);
    if (!made) return null;
    const [c, ctx] = made;
    const centre = Math.round((w - 544) / 2);
    for (const id of ['mountainsFar', 'mountains', 'trees', 'treesFront'] as const) {
      const img = layers.get(id);
      if (img) tile(ctx, img, centre, w);
    }
    // Meadow: ragged grass edge against the treeline, then sparse blades.
    ctx.fillStyle = '#04060c';
    ctx.fillRect(0, MEADOW_Y, w, L_H - MEADOW_Y);
    for (let x = 0; x < w; x++) {
      const tip = Math.floor(rnd() * 3);
      if (tip > 0) ctx.fillRect(x, MEADOW_Y - tip, 1, tip);
    }
    ctx.fillStyle = '#070a13';
    for (let y = MEADOW_Y; y < MEADOW_Y + 5; y++) {
      for (let x = (y % 2) * 2; x < w; x += 4) ctx.fillRect(x, y, 1, 1);
    }
    for (let i = 0; i < w * 0.9; i++) {
      const x = Math.floor(rnd() * w);
      const y = MEADOW_Y + 3 + Math.floor(rnd() * (L_H - MEADOW_Y - 5));
      const lit = rnd() < 0.25;
      ctx.fillStyle = lit ? '#141c2c' : '#0b0f1a';
      ctx.fillRect(x, y, 1, 1);
      if (lit && rnd() < 0.5) {
        ctx.fillRect(x - 1, y + 1, 1, 1);
        ctx.fillRect(x + 1, y + 1, 1, 1);
      }
    }
    return c;
  }

  function build(w: number): Scene {
    const rnd = mulberry(0x3a7);
    const moonX = Math.round(w * 0.78);
    const back = paintBack(w, moonX);
    const front = paintFront(w, rnd);

    // Stars only where the sky is open — never on a cloud or the moon.
    const open = back?.getContext('2d')?.getImageData(0, 0, w, 112).data;
    const stars: Star[] = [];
    for (let i = 0; i < Math.round(w * 0.55); i++) {
      const x = Math.floor(rnd() * w);
      const y = Math.floor(rnd() * 110);
      const b = 0.35 + rnd() * 0.65;
      const ph = rnd() * Math.PI * 2;
      const sp = 0.4 + rnd() * 1.4;
      const big = rnd() < 0.05;
      if (open) {
        const o = (y * w + x) * 4;
        if (!OPEN_SKY.has(`${open[o]},${open[o + 1]},${open[o + 2]}`)) continue;
      }
      stars.push({ x, y, b: y > 80 ? b * 0.6 : b, ph, sp, big });
    }

    const flies: Fly[] = [];
    for (let i = 0; i < 7; i++) {
      flies.push({
        x0: w * 0.12 + rnd() * w * 0.76,
        y0: MEADOW_Y + 6 + rnd() * (L_H - MEADOW_Y - 18),
        rx: 6 + rnd() * 14,
        ry: 2 + rnd() * 5,
        ph: rnd() * Math.PI * 2,
        sp: 0.25 + rnd() * 0.5,
      });
    }

    const pals: Pal[] = [
      { sheet: 'cat', x: Math.round(w * 0.5), base: L_H - 7, fps: 1.1, scale: 0.75, zzz: true },
    ];
    return { w, moonX, stars, flies, pals, back, front };
  }

  function draw(ctx: CanvasRenderingContext2D, s: Scene, t: number): void {
    const { w } = s;
    ctx.imageSmoothingEnabled = false;

    if (s.back) ctx.drawImage(s.back, 0, 0);
    else {
      ctx.fillStyle = '#090b18';
      ctx.fillRect(0, 0, w, L_H);
    }

    for (const st of s.stars) {
      const a = st.b * (0.55 + 0.45 * Math.sin(t * 0.001 * st.sp + st.ph));
      ctx.fillStyle = `rgba(226, 230, 245, ${a.toFixed(3)})`;
      ctx.fillRect(st.x, st.y, 1, 1);
      if (st.big) {
        ctx.fillRect(st.x - 1, st.y, 1, 1);
        ctx.fillRect(st.x + 1, st.y, 1, 1);
        ctx.fillRect(st.x, st.y - 1, 1, 1);
        ctx.fillRect(st.x, st.y + 1, 1, 1);
      }
    }

    // A shooting star crosses the upper sky every so often.
    const cycle = 17000;
    const n = Math.floor(t / cycle);
    const k = (t % cycle) / 650;
    if (k < 1) {
      const sx = Math.round(w * (0.12 + ((n * 37) % 50) / 100) + k * 46);
      const sy = Math.round(8 + ((n * 13) % 22) + k * 17);
      for (let i = 0; i < 7; i++) {
        ctx.fillStyle = `rgba(236, 240, 255, ${((1 - i / 7) * (1 - k) * 0.9).toFixed(3)})`;
        ctx.fillRect(sx - i * 2, sy - Math.round(i * 0.74), 1, 1);
      }
    }

    if (s.front) ctx.drawImage(s.front, 0, 0);

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
    )
      .then(() =>
        Promise.all(
          (Object.keys(LAYER_FILES) as LayerId[]).map(
            (id) =>
              new Promise<void>((resolve) => {
                const img = new Image();
                img.onload = () => {
                  layers.set(id, img);
                  resolve();
                };
                img.onerror = () => resolve();
                img.src = LAYER_FILES[id];
              }),
          ),
        ),
      )
      .then(() => {
        scene = build(canvas?.width ?? 300);
        draw(ctx, scene, performance.now());
      });

    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    const resize = () => {
      if (!canvas) return;
      const rect = canvas.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      canvas.height = L_H;
      const width = Math.max(200, Math.round((rect.width / rect.height) * L_H));
      // Setting a canvas size clears it, so only touch it on a real change.
      if (canvas.width === width && canvas.height === L_H && scene.w === width) return;
      canvas.height = L_H;
      canvas.width = width;
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
