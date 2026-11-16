#!/usr/bin/env node
// Contrast audit for the glass layer.
//
// Frosted panes are translucent, so small text can lose contrast against the
// ambient orbs behind them. This recomputes, from palettes.json and the glass
// alphas in app.css, the WCAG ratio of every palette's text tokens over the
// brightest possible pane pixel — the worst case is `--bg-0` with an accent orb
// on top, then the pane fill over that.
//
//   node scripts/audit-contrast.mjs          # report
//   node scripts/audit-contrast.mjs --check  # fail if glass costs too much
//
// The invariant is relative, not absolute: glass may not take more than 8% of
// the contrast the same text has on a solid surface. `--fg-3` is reported for
// information — it is a deliberately quiet tone that already sits under 3:1 on
// opaque surfaces in most palettes.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const src = join(root, 'apps', 'desktop', 'src');
const palettes = JSON.parse(readFileSync(join(src, 'lib', 'palettes.json'), 'utf8'));
const css = readFileSync(join(src, 'app.css'), 'utf8');

const pct = (pattern, what) => {
  const hit = css.match(pattern);
  if (!hit) throw new Error(`could not read ${what} from app.css`);
  return Number(hit[1]) / 100;
};

// Dark tokens come first, then the light-palette override block.
const glowDark = pct(/--glass-glow:\s*(\d+)%/, 'glass glow');
const glows = [...css.matchAll(/--glass-glow:\s*(\d+)%/g)].map((m) => Number(m[1]) / 100);
const glowLight = glows.at(-1);
const fills = [...css.matchAll(/--glass-fill:\s*color-mix\(in srgb,\s*var\(--glass-base\)\s*(\d+)%/g)].map(
  (m) => Number(m[1]) / 100,
);
const fillDark = fills[0];
const fillLight = fills.at(-1);

function color(text) {
  const value = text.trim();
  if (value.startsWith('#')) {
    const hex = value.slice(1);
    const full = hex.length === 3 ? [...hex].map((c) => c + c).join('') : hex;
    return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16)).concat(1);
  }
  const m = value.match(/rgba?\(([^)]+)\)/);
  if (!m) throw new Error(`unsupported colour: ${value}`);
  const parts = m[1].split(',').map((p) => p.trim());
  const [r, g, b] = parts.map(Number);
  const a = parts[3] === undefined ? 1 : Number(parts[3]);
  return [r, g, b, a];
}

const over = (fg, bg) => [
  fg[0] * fg[3] + bg[0] * (1 - fg[3]),
  fg[1] * fg[3] + bg[1] * (1 - fg[3]),
  fg[2] * fg[3] + bg[2] * (1 - fg[3]),
  1,
];

function luminance([r, g, b]) {
  const channel = (v) => {
    const s = v / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrast(a, b) {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

const rows = [];
for (const theme of palettes.themes) {
  if (!theme.tokens) continue;
  const t = theme.tokens;
  const light = theme.scheme === 'light';
  const bg0 = color(t['--bg-0']);
  const bg1 = color(t['--bg-1']);
  const accent = color(t['--accent']);
  const ambient = over([accent[0], accent[1], accent[2], light ? glowLight : glowDark], bg0);
  const pane = over([bg1[0], bg1[1], bg1[2], light ? fillLight : fillDark], ambient);

  const row = { id: theme.id, light };
  for (const name of ['--fg-1', '--fg-2', '--fg-3']) {
    row[name] = {
      solid: contrast(color(t[name]), bg1),
      glass: contrast(color(t[name]), pane),
    };
  }
  rows.push(row);
}

rows.sort((a, b) => a['--fg-3'].glass - b['--fg-3'].glass);

const pad = (v) => v.toFixed(2).padStart(6);
console.log(
  `glass fill: ${Math.round(fillDark * 100)}% dark / ${Math.round(fillLight * 100)}% light   ` +
    `orbs: ${Math.round(glowDark * 100)}% dark / ${Math.round(glowLight * 100)}% light`,
);
console.log(`${'palette'.padEnd(13)}${'fg-2 solid'.padStart(9)}${'glass'.padStart(7)}${'fg-3 solid'.padStart(11)}${'glass'.padStart(7)}`);
for (const row of rows) {
  console.log(
    `${row.id.padEnd(13)}${pad(row['--fg-2'].solid)}${pad(row['--fg-2'].glass)}` +
      `${pad(row['--fg-3'].solid)}${pad(row['--fg-3'].glass)}`,
  );
}

const FLOOR = 0.92;
const offenders = rows.filter((r) => r['--fg-1'].glass < r['--fg-1'].solid * FLOOR || r['--fg-2'].glass < r['--fg-2'].solid * FLOOR);
if (offenders.length) {
  console.error(
    `\n${offenders.length} palette(s) lose more than ${Math.round((1 - FLOOR) * 100)}% of fg-1/fg-2 contrast on glass: ` +
      offenders.map((r) => r.id).join(', '),
  );
  if (process.argv.includes('--check')) process.exit(1);
} else {
  console.log('\nglass keeps fg-1/fg-2 within 8% of their solid-surface contrast in every palette');
}
