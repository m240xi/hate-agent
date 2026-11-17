export interface HighlightToken {
  text: string;
  cls?: string;
}

const KEYWORDS = new Set([
  'fn', 'let', 'mut', 'const', 'pub', 'struct', 'enum', 'impl', 'trait', 'for', 'in',
  'if', 'else', 'match', 'while', 'loop', 'return', 'break', 'continue', 'use', 'mod',
  'crate', 'self', 'super', 'async', 'await', 'move', 'where', 'type', 'as', 'ref',
  'function', 'var', 'import', 'export', 'from', 'default', 'class', 'extends',
  'interface', 'implements', 'new', 'try', 'catch', 'finally', 'throw', 'typeof',
  'keyof', 'readonly', 'true', 'false', 'null', 'undefined', 'None', 'Some', 'Ok', 'Err',
  'def', 'elif', 'pass', 'with', 'lambda', 'yield', 'raise', 'except', 'global', 'nonlocal',
  'True', 'False', 'and', 'or', 'not', 'is', 'select', 'where', 'join', 'order', 'group',
  'insert', 'update', 'delete', 'create', 'table', 'alter', 'drop', 'into', 'values',
  'script', 'style', 'state', 'derived', 'effect',
]);

export function languageForPath(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  switch (ext) {
    case 'rs': return 'rust';
    case 'ts': case 'mts': case 'tsx': return 'ts';
    case 'js': case 'mjs': case 'jsx': return 'js';
    case 'svelte': case 'html': case 'xml': case 'svg': return 'markup';
    case 'py': return 'python';
    case 'json': case 'jsonc': return 'json';
    case 'css': case 'scss': return 'css';
    case 'sh': case 'bash': case 'zsh': case 'ps1': return 'shell';
    case 'sql': return 'sql';
    case 'md': case 'mdx': return 'markdown';
    case 'toml': case 'yaml': case 'yml': return 'config';
    default: return 'plain';
  }
}

export function highlightLine(line: string, lang: string): HighlightToken[] {
  if (!line || lang === 'plain') return [{ text: line }];

  if (lang === 'markdown') {
    if (/^\s*#{1,6}\s/.test(line)) {
      return [{ text: line, cls: 'hl-kw' }];
    }
    if (/^\s*(?:[-*+]|\d+\.)\s/.test(line)) {
      return [{ text: line, cls: 'hl-type' }];
    }
    return [{ text: line }];
  }

  const out: HighlightToken[] = [];
  const push = (text: string, cls?: string): void => {
    const last = out[out.length - 1];
    if (last && last.cls === cls) {
      last.text += text;
    } else if (cls) {
      out.push({ text, cls });
    } else {
      out.push({ text });
    }
  };

  let i = 0;
  const n = line.length;

  while (i < n) {
    const ch = line[i]!;
    const rest = line.slice(i);

    if (
      (ch === '/' && line[i + 1] === '/') ||
      (ch === '#' && (lang === 'python' || lang === 'shell' || lang === 'config')) ||
      (ch === '-' && line[i + 1] === '-' && lang === 'sql')
    ) {
      push(rest, 'hl-com');
      break;
    }

    if (ch === '"' || ch === "'" || ch === '`') {
      let j = i + 1;
      while (j < n) {
        if (line[j] === '\\') {
          j += 2;
          continue;
        }
        if (line[j] === ch) {
          j++;
          break;
        }
        j++;
      }
      const token = line.slice(i, j);
      if (lang === 'json' && ch === '"' && /^\s*:/.test(line.slice(j))) {
        push(token, 'hl-type');
      } else {
        push(token, 'hl-str');
      }
      i = j;
      continue;
    }

    if (/[0-9]/.test(ch) && (i === 0 || !/[A-Za-z0-9_]/.test(line[i - 1]!))) {
      const m = rest.match(/^(?:0x[0-9a-fA-F_]+|[0-9][0-9_]*(?:\.[0-9_]+)?)/);
      if (m) {
        push(m[0], 'hl-num');
        i += m[0].length;
        continue;
      }
    }

    if (/[A-Za-z_$]/.test(ch)) {
      const m = rest.match(/^[A-Za-z0-9_$]+/);
      if (m) {
        const word = m[0];
        if (KEYWORDS.has(word)) {
          push(word, 'hl-kw');
        } else if (/^[A-Z][A-Za-z0-9_]*$/.test(word)) {
          push(word, 'hl-type');
        } else {
          push(word);
        }
        i += word.length;
        continue;
      }
    }

    if (lang === 'markup' && ch === '<') {
      const m = rest.match(/^<\/?[A-Za-z0-9:_-]+/);
      if (m) {
        push(m[0], 'hl-kw');
        i += m[0].length;
        continue;
      }
    }

    push(ch);
    i++;
  }

  return out;
}
