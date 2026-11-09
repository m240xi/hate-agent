const ESCAPES: Record<string, string> = {
  '&': '&amp;',
  '<': '&lt;',
  '>': '&gt;',
  '"': '&quot;',
};

function esc(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ESCAPES[c] ?? c);
}

function safeHref(url: string): string | null {
  const u = url.trim();
  if (/^(https?:\/\/|mailto:|\/|#)/i.test(u)) return u;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(u)) return u;
  return null;
}

function inline(text: string): string {
  const parts = text.split(/(`[^`\n]+`)/g);
  return parts
    .map((p, i) =>
      i % 2 === 1 ? `<code>${p.slice(1, -1)}</code>` : formatInline(p),
    )
    .join('');
}

function formatInline(s: string): string {
  s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/__([^_]+)__/g, '<strong>$1</strong>');
  s = s.replace(/\*([^*\n]+)\*/g, '<em>$1</em>');
  s = s.replace(/(^|[^\w])_([^_\n]+)_(?!\w)/g, '$1<em>$2</em>');
  s = s.replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
  s = s.replace(
    /\[([^\]]+)\]\(([^)\s]+)\)/g,
    (m, label: string, url: string) => {
      const href = safeHref(url);
      return href
        ? `<a href="${href}" target="_blank" rel="noopener">${label}</a>`
        : m;
    },
  );
  return s;
}

function isBlank(line: string): boolean {
  return line.trim() === '';
}

function renderBlocks(md: string): string {
  const lines = md.replace(/\r\n?/g, '\n').split('\n');
  const out: string[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i] ?? '';

    const fence = line.match(/^```(\w*)\s*$/);
    if (fence) {
      const buf: string[] = [];
      i++;
      while (i < lines.length && !(lines[i] ?? '').startsWith('```')) {
        buf.push(lines[i] ?? '');
        i++;
      }
      i++;
      const lang = fence[1] ? ` class="lang-${esc(fence[1])}"` : '';
      out.push(
        `<pre><button type="button" class="copy-code" aria-label="Copy code">copy</button><code${lang}>${esc(buf.join('\n'))}</code></pre>`,
      );
      continue;
    }

    if (isBlank(line)) {
      i++;
      continue;
    }

    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      const level = Math.min(h[1]?.length ?? 1, 4);
      out.push(`<h${level}>${inline(esc(h[2] ?? ''))}</h${level}>`);
      i++;
      continue;
    }

    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      out.push('<hr>');
      i++;
      continue;
    }

    if (/^\s*>/.test(line)) {
      const buf: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i] ?? '')) {
        buf.push((lines[i] ?? '').replace(/^\s*>\s?/, ''));
        i++;
      }
      out.push(`<blockquote>${inline(esc(buf.join('\n'))).replace(/\n/g, '<br>')}</blockquote>`);
      continue;
    }

    const ulItem = line.match(/^\s*[-*+]\s+(.*)$/);
    const olItem = line.match(/^\s*\d+[.)]\s+(.*)$/);
    if (ulItem || olItem) {
      const ordered = !!olItem;
      const tag = ordered ? 'ol' : 'ul';
      const items: string[] = [];
      const itemRe = ordered ? /^\s*\d+[.)]\s+(.*)$/ : /^\s*[-*+]\s+(.*)$/;
      while (i < lines.length) {
        const m = (lines[i] ?? '').match(itemRe);
        if (!m) break;
        items.push(`<li>${inline(esc(m[1] ?? ''))}</li>`);
        i++;
      }
      out.push(`<${tag}>${items.join('')}</${tag}>`);
      continue;
    }

    const buf: string[] = [];
    while (
      i < lines.length &&
      !isBlank(lines[i] ?? '') &&
      !/^```/.test(lines[i] ?? '') &&
      !/^#{1,6}\s/.test(lines[i] ?? '') &&
      !/^\s*>/.test(lines[i] ?? '') &&
      !/^\s*[-*+]\s/.test(lines[i] ?? '') &&
      !/^\s*\d+[.)]\s/.test(lines[i] ?? '') &&
      !/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(lines[i] ?? '')
    ) {
      buf.push(lines[i] ?? '');
      i++;
    }
    out.push(`<p>${inline(esc(buf.join('\n'))).replace(/\n/g, '<br>')}</p>`);
  }

  return out.join('');
}

export function renderMarkdown(md: string): string {
  if (!md.trim()) return '';
  return renderBlocks(md);
}
