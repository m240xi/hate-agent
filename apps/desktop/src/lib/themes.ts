// Theme registry — ids match `data-theme` values in app.css. The
// preview colors are approximations of each palette's bg-1/accent/fg-0
// for the Settings picker chips; keep them in sync when tuning themes.
export interface ThemeMeta {
  id: string;
  label: string;
  hint: string;
  /** Preview swatch colors. */
  bg: string;
  accent: string;
  fg: string;
}

export const THEMES: ThemeMeta[] = [
  {
    id: 'auto',
    label: 'Auto',
    hint: 'follow the system',
    bg: 'linear-gradient(135deg,#0a0a0c 50%,#edebe5 50%)',
    accent: '#8d8a82',
    fg: '#f0eee9',
  },
  {
    id: 'dark',
    label: 'Midnight',
    hint: 'OLED black, warm mono',
    bg: '#0a0a0c',
    accent: '#efede6',
    fg: '#f0eee9',
  },
  {
    id: 'light',
    label: 'Daylight',
    hint: 'warm paper light',
    bg: '#ffffff',
    accent: '#1c1b18',
    fg: '#1a1917',
  },
  {
    id: 'phosphor',
    label: 'Phosphor',
    hint: 'terminal green',
    bg: '#071009',
    accent: '#8fe6a8',
    fg: '#e2f5e4',
  },
  {
    id: 'ember',
    label: 'Ember',
    hint: 'campfire amber',
    bg: '#140d09',
    accent: '#f2a95e',
    fg: '#f5ebe0',
  },
  {
    id: 'sakura',
    label: 'Sakura',
    hint: 'night-flower pink',
    bg: '#110c18',
    accent: '#e8a0c4',
    fg: '#f2ecf7',
  },
];

export function isThemeId(v: string | null): v is string {
  return v != null && THEMES.some((t) => t.id === v);
}
