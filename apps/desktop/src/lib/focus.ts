/**
 * Keep Tab focus inside a dialog-ish element and hand focus back to whatever
 * was focused before it opened.
 */
export function trapFocus(node: HTMLElement): { destroy: () => void } {
  const previous = document.activeElement as HTMLElement | null;

  const selector = [
    'a[href]',
    'button:not([disabled])',
    'input:not([disabled])',
    'select:not([disabled])',
    'textarea:not([disabled])',
    '[tabindex]:not([tabindex="-1"])',
  ].join(',');

  function focusables(): HTMLElement[] {
    return [...node.querySelectorAll<HTMLElement>(selector)].filter(
      (el) => el.offsetParent !== null || el === document.activeElement,
    );
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key !== 'Tab') return;
    const items = focusables();
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;
    if (!first || !last) return;
    if (e.shiftKey && (active === first || !node.contains(active))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  }

  node.addEventListener('keydown', onKeydown);
  return {
    destroy() {
      node.removeEventListener('keydown', onKeydown);
      if (previous && document.contains(previous)) previous.focus();
    },
  };
}

/** Arrow-key navigation for a listbox-style list. */
export function arrowNav(
  node: HTMLElement,
  opts: { onMove: (delta: number) => void; onEnter?: () => void; onEscape?: () => void },
): { destroy: () => void } {
  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      opts.onMove(1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      opts.onMove(-1);
    } else if (e.key === 'Enter' && opts.onEnter) {
      e.preventDefault();
      opts.onEnter();
    } else if (e.key === 'Escape' && opts.onEscape) {
      e.preventDefault();
      opts.onEscape();
    }
  }
  node.addEventListener('keydown', onKeydown);
  return {
    destroy() {
      node.removeEventListener('keydown', onKeydown);
    },
  };
}
