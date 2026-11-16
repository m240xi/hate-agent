export interface ThemeMeta {
  id: string;
  label: string;
  hint: string;
  bg: string;
  accent: string;
  fg: string;
  scheme?: 'dark' | 'light';
}

import palettes from './palettes.json';

export interface Palette {
  id: string;
  label: string;
  hint: string;
  scheme: 'auto' | 'dark' | 'light';
  swatch: { bg: string; accent: string; fg: string };
  /** The CSS custom properties emitted for this palette in styles/themes.css. */
  tokens?: Record<string, string>;
}

const DATA = palettes as { themes: Palette[] };

/**
 * The grid reads its swatches and labels straight from the palette table, and
 * `scripts/gen-themes.mjs` emits the matching token block from the same file —
 * so a theme can't drift between its preview tile and its colours.
 */
export const THEMES: ThemeMeta[] = DATA.themes.map((p) => ({
  id: p.id,
  label: p.label,
  hint: p.hint,
  bg: p.swatch.bg,
  accent: p.swatch.accent,
  fg: p.swatch.fg,
  ...(p.scheme === 'auto' ? {} : { scheme: p.scheme }),
}));

/** Palettes that define a token block (everything except the Auto entry). */
export const PALETTES: Palette[] = DATA.themes.filter(
  (p): p is Palette & { tokens: Record<string, string> } => p.tokens != null,
);

export function isThemeId(v: string | null): v is string {
  return v != null && THEMES.some((t) => t.id === v);
}
