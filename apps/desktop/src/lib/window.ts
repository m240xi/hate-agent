const inTauri = (): boolean => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

async function current() {
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  return getCurrentWindow();
}

export function isDesktop(): boolean {
  return inTauri();
}

export async function dragRegion(e: MouseEvent): Promise<void> {
  if (!inTauri() || e.button !== 0) return;
  const el = e.target as HTMLElement;
  if (el.closest('button, a, input, textarea, select, [role="button"], [data-no-drag]')) return;
  const win = await current();
  if (e.detail === 2) await win.toggleMaximize();
  else await win.startDragging();
}

export async function minimize(): Promise<void> {
  await (await current()).minimize();
}

export async function toggleMaximize(): Promise<void> {
  await (await current()).toggleMaximize();
}

export async function close(): Promise<void> {
  await (await current()).close();
}
