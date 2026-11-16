/**
 * One context menu for the whole app.
 *
 * WebView2's default menu (Reload, Save as, Print, Inspect) is suppressed
 * everywhere except real text fields, where it is still the only reliable way to
 * paste and it carries spellcheck. Everywhere else the menu is built from the
 * element under the cursor: anything that wants a menu marks itself with
 * `data-ctx="<kind>"` (+ payload in `data-ctx-*` attributes), and the builder
 * below turns that kind into actions that already exist in the store.
 */
import { session } from './store.svelte';
import { copyText } from './format';

export interface MenuAction {
  id: string;
  label: string;
  /** shortcut or muted detail, right-aligned */
  hint?: string;
  icon?: string;
  danger?: boolean;
  disabled?: boolean;
  /** any result is ignored; copyText resolves to a boolean, for instance */
  run: () => unknown;
}

export type MenuEntry = MenuAction | { sep: true };

interface MenuState {
  open: boolean;
  x: number;
  y: number;
  entries: MenuEntry[];
}

export const ctxMenu = $state<MenuState>({ open: false, x: 0, y: 0, entries: [] });

let lastFocus: HTMLElement | null = null;

/**
 * Closes the menu. `restore` puts focus back where it was — used for Escape,
 * so cancelling a context menu never costs you your place in the composer.
 */
export function closeMenu(restore = false): void {
  const target = lastFocus;
  lastFocus = null;
  if (ctxMenu.open) ctxMenu.open = false;
  if (ctxMenu.entries.length > 0) ctxMenu.entries = [];
  if (restore && target && target.isConnected) target.focus();
}

function openAt(x: number, y: number, entries: MenuEntry[]): void {
  if (entries.length === 0) {
    closeMenu();
    return;
  }
  lastFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  ctxMenu.entries = entries;
  ctxMenu.x = x;
  ctxMenu.y = y;
  ctxMenu.open = true;
}

const sep: { sep: true } = { sep: true };

/** Assets in the transcript are URLs or data URLs, so copying is best effort. */
async function copyImage(src: string): Promise<void> {
  try {
    const blob = await (await fetch(src)).blob();
    const type = blob.type || 'image/png';
    await navigator.clipboard.write([new ClipboardItem({ [type]: blob })]);
    session.note('Image copied');
  } catch {
    session.note('Could not copy the image — the clipboard refused the write');
  }
}

function emit(event: string, detail: Record<string, unknown>): void {
  window.dispatchEvent(new CustomEvent(event, { detail }));
}

function blockById(id: number) {
  return session.blocks.find((b) => b.id === id);
}

function messageActions(el: HTMLElement): MenuEntry[] {
  const kind = el.dataset.kind ?? 'text';
  const id = Number(el.dataset.block ?? '');
  const block = Number.isFinite(id) ? blockById(id) : undefined;
  const raw = block?.text ?? '';
  // the target may be the text element itself (an error block), so fall back to it
  const shown = el.querySelector('.assistant-text, .user-text, .thinking-body, .error-msg') ?? el;
  const shownText = shown.textContent?.trim() ?? '';

  const out: MenuEntry[] = [];
  if (shownText) {
    out.push({ id: 'copy', label: 'Copy', icon: 'copy', hint: 'Ctrl+C', run: () => copyText(shownText) });
  }
  if (raw && raw !== shownText) {
    out.push({ id: 'copy-md', label: 'Copy as Markdown', icon: 'copy', run: () => copyText(raw) });
  }
  if (kind === 'user' && Number.isFinite(id)) {
    out.push(sep, {
      id: 'edit',
      label: 'Edit message',
      icon: 'edit',
      run: () => emit('hate:edit-message', { id }),
    });
  }
  if (kind === 'text' && session.canRegenerate) {
    out.push(sep, {
      id: 'regen',
      label: 'Regenerate reply',
      icon: 'refresh',
      disabled: session.running,
      run: () => session.regenerate(),
    });
  }
  return out;
}

function toolActions(el: HTMLElement): MenuEntry[] {
  const copy = el.dataset.copy ?? '';
  const path = el.dataset.path ?? '';
  const writer = el.dataset.writer === 'true';
  const out: MenuEntry[] = [];
  if (copy) {
    out.push({ id: 'copy-target', label: 'Copy path or command', icon: 'copy', run: () => copyText(copy) });
  }
  out.push({
    id: 'copy-name',
    label: 'Copy tool name',
    run: () => copyText(el.dataset.name ?? ''),
  });
  if (path) {
    out.push(sep, {
      id: 'open',
      label: 'Open file',
      icon: 'file',
      run: () => session.openFile(path),
    });
    if (writer) {
      out.push({ id: 'diff', label: 'Open full diff', icon: 'diff', run: () => session.openDiff(path) });
    }
  }
  return out;
}

function codeActions(el: HTMLElement): MenuEntry[] {
  const code = el.querySelector('code')?.textContent ?? '';
  const lang = el.dataset.lang ?? '';
  if (!code) return [];
  return [
    { id: 'copy-code', label: 'Copy code', icon: 'copy', run: () => copyText(code) },
    {
      id: 'copy-fence',
      label: 'Copy as Markdown',
      run: () => copyText(`\`\`\`${lang}\n${code}\n\`\`\``),
    },
  ];
}

function sessionActions(el: HTMLElement): MenuEntry[] {
  const id = el.dataset.id ?? '';
  const title = el.dataset.title ?? '';
  if (!id) return [];
  return [
    { id: 'open', label: 'Open chat', icon: 'chevron-right', run: () => session.openSession(id) },
    { id: 'rename', label: 'Rename', icon: 'edit', run: () => emit('hate:rename-session', { id }) },
    { id: 'copy-title', label: 'Copy title', icon: 'copy', run: () => copyText(title) },
    sep,
    { id: 'delete', label: 'Delete chat', icon: 'trash', danger: true, run: () => session.deleteSession(id) },
  ];
}

function fileActions(el: HTMLElement): MenuEntry[] {
  const path = el.dataset.path ?? '';
  if (!path) return [];
  const pinned = session.isPinned(path);
  const isDir = el.dataset.kind === 'dir';
  const out: MenuEntry[] = [];
  if (!isDir) {
    out.push({ id: 'open', label: 'Open', icon: 'file', run: () => session.openFile(path) });
  }
  out.push({ id: 'copy-path', label: 'Copy path', icon: 'copy', run: () => copyText(path) });
  if (!isDir) {
    out.push(sep, {
      id: 'pin',
      label: pinned ? 'Detach from next message' : 'Attach to next message',
      icon: 'paperclip',
      run: () => session.togglePin(path),
    });
  }
  return out;
}

function imageActions(el: HTMLElement): MenuEntry[] {
  const src = el.getAttribute('src') ?? el.dataset.src ?? '';
  if (!src) return [];
  return [{ id: 'copy-image', label: 'Copy image', icon: 'image', run: () => copyImage(src) }];
}

function transcriptActions(): MenuEntry[] {
  const entries: MenuEntry[] = [];
  if (session.blocks.length > 0) {
    entries.push({
      id: 'export',
      label: 'Export transcript…',
      icon: 'download',
      run: () => session.exportTranscript(),
    });
    entries.push(sep);
    entries.push({
      id: 'clear',
      label: 'Clear transcript',
      icon: 'trash',
      danger: true,
      run: () => session.clearTranscript(),
    });
  }
  if (entries.length > 0) entries.push(sep);
  entries.push({
    id: 'compact',
    label: 'Compact context',
    icon: 'sparkles',
    disabled: session.running,
    run: () => session.compact(),
  });
  return entries;
}

/** In a text field the platform menu is the better one: it can paste. */
function isTextEntry(el: HTMLElement): boolean {
  return !!el.closest('input, textarea, [contenteditable=""], [contenteditable="true"]');
}

function build(el: HTMLElement, at: { x: number; y: number }): MenuEntry[] {
  const owner = el.closest<HTMLElement>('[data-ctx]');
  let entries: MenuEntry[] = [];

  switch (owner?.dataset.ctx) {
    case 'code':
      entries = codeActions(owner);
      break;
    case 'message':
      entries = messageActions(owner);
      break;
    case 'tool':
      entries = toolActions(owner);
      break;
    case 'session':
      entries = sessionActions(owner);
      break;
    case 'file':
      entries = fileActions(owner);
      break;
    case 'image':
      entries = imageActions(owner);
      break;
    case 'transcript':
      entries = transcriptActions();
      break;
    default:
      break;
  }

  // A link with no menu of its own still offers the only useful action.
  const link = el.closest<HTMLAnchorElement>('a[href]');
  if (entries.length === 0 && link) {
    const href = link.href;
    entries = [{ id: 'copy-link', label: 'Copy link', icon: 'link', run: () => copyText(href) }];
  }

  // Text selected anywhere in the page: copying it comes first.
  const sel = window.getSelection();
  const selected = sel && !sel.isCollapsed ? sel.toString().trim() : '';
  if (selected) {
    const lead: MenuEntry[] = [
      { id: 'copy-selection', label: 'Copy selection', icon: 'copy', hint: 'Ctrl+C', run: () => copyText(selected) },
    ];
    entries = entries.length > 0 ? [...lead, sep, ...entries] : lead;
  }

  if (entries.length === 0) return [];

  // Clamp the click point so the panel's own corners stay on screen.
  void at;
  return entries;
}

/**
 * Installs the global handler. Returns a teardown for `$effect`/`onDestroy`.
 * Must run from the component that renders <ContextMenu />.
 */
export function installContextMenu(): () => void {
  if (typeof window === 'undefined') return () => {};

  const onContextMenu = (event: MouseEvent): void => {
    const el = event.target;
    if (!(el instanceof HTMLElement)) return;
    // Keep the platform menu where it is genuinely better.
    if (isTextEntry(el)) {
      closeMenu();
      return;
    }
    event.preventDefault();

    // Keyboard-invoked menus (Shift+F10 / the menu key) carry no coordinates.
    let x = event.clientX;
    let y = event.clientY;
    if (x === 0 && y === 0) {
      const r = (el.closest('[data-ctx]') ?? el).getBoundingClientRect();
      x = Math.round(r.left + Math.min(24, r.width / 2));
      y = Math.round(r.top + Math.min(24, r.height / 2));
    }

    const entries = build(el, { x, y });
    if (entries.length === 0) {
      closeMenu();
      return;
    }
    openAt(x, y, entries);
  };

  const onPointerDown = (event: PointerEvent): void => {
    if (!ctxMenu.open) return;
    const el = event.target;
    if (el instanceof Element && el.closest('.ctx-menu')) return;
    closeMenu();
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    if (event.key === 'Escape' && ctxMenu.open) {
      event.preventDefault();
      closeMenu(true);
    }
  };

  const onDismiss = (): void => closeMenu();

  window.addEventListener('contextmenu', onContextMenu);
  window.addEventListener('pointerdown', onPointerDown, true);
  window.addEventListener('keydown', onKeyDown);
  window.addEventListener('resize', onDismiss);
  window.addEventListener('blur', onDismiss);
  window.addEventListener('scroll', onDismiss, true);
  window.addEventListener('wheel', onDismiss, { capture: true, passive: true });

  return () => {
    window.removeEventListener('contextmenu', onContextMenu);
    window.removeEventListener('pointerdown', onPointerDown, true);
    window.removeEventListener('keydown', onKeyDown);
    window.removeEventListener('resize', onDismiss);
    window.removeEventListener('blur', onDismiss);
    window.removeEventListener('scroll', onDismiss, true);
    window.removeEventListener('wheel', onDismiss, true);
  };
}
