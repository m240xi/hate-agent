import type { PlanStep, TokenUsage, TranscriptBlock, TurnSummary } from './types';

export function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export function usageLabel(u: TokenUsage, withThinking = true): string {
  const cached = u.cached_input > 0 ? ` (${fmtTok(u.cached_input)} cached)` : '';
  const think = withThinking && u.thinking > 0 ? ` · think ${fmtTok(u.thinking)}` : '';
  return `in ${fmtTok(u.input)}${cached} · out ${fmtTok(u.output)}${think}`;
}

export function usageTitle(u: TokenUsage): string {
  const fresh = Math.max(0, u.input - u.cached_input);
  return [
    `input ${u.input.toLocaleString()} tokens`,
    `  ${u.cached_input.toLocaleString()} from cache, ${fresh.toLocaleString()} fresh`,
    `output ${u.output.toLocaleString()} tokens`,
    u.thinking > 0 ? `thinking ${u.thinking.toLocaleString()} tokens` : '',
  ]
    .filter(Boolean)
    .join('\n');
}

function fmtElapsed(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
}

export function turnLabel(t: TurnSummary): string {
  const parts = [usageLabel(t.usage)];
  if (t.toolCalls > 0) parts.push(`${t.toolCalls} tool${t.toolCalls === 1 ? '' : 's'}`);
  parts.push(fmtElapsed(t.elapsedMs));
  if (t.costUsd !== null && t.costUsd > 0) parts.push(`$${t.costUsd.toFixed(4)}`);
  return parts.join(' · ');
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand('copy');
      ta.remove();
      return ok;
    } catch {
      return false;
    }
  }
}

export function parsePlan(text: string): PlanStep[] {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const pick = (re: RegExp): string[] =>
    lines
      .map((l) => re.exec(l)?.[1]?.trim() ?? '')
      .filter((l) => l.length > 0);
  let steps = pick(/^\s{0,3}\d+[.)]\s+(.*)$/);
  if (steps.length === 0) steps = pick(/^[-*]\s+(.*)$/);
  return steps.slice(0, 30).map((raw) => {
    const clean = raw.replace(/\*\*|__|`/g, '');
    return {
      description: clean.length > 200 ? `${clean.slice(0, 200)}…` : clean,
      status: 'pending' as const,
    };
  });
}

export function transcriptMarkdown(blocks: TranscriptBlock[]): string {
  let out = '';
  let lastWasTool = false;
  const push = (chunk: string, isTool: boolean): void => {
    if (out) out += isTool && lastWasTool ? '\n' : '\n\n';
    out += chunk;
    lastWasTool = isTool;
  };
  for (const b of blocks) {
    if (b.kind === 'user') push(`## You\n\n${b.text ?? ''}`, false);
    else if (b.kind === 'text') push(b.text ?? '', false);
    else if (b.kind === 'error') push(`> ${b.text ?? ''}`, false);
    else if (b.kind === 'tool' && b.call) {
      const a = b.call.args;
      const target =
        typeof a.path === 'string'
          ? a.path
          : typeof a.command === 'string'
            ? a.command
            : typeof a.pattern === 'string'
              ? a.pattern
              : typeof a.query === 'string'
                ? a.query
                : '';
      push(`- \`${b.call.name}\` ${target}`.trimEnd(), true);
    }
  }
  return out;
}

function esc(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export interface TranscriptExportMeta {
  title: string;
  model: string;
  provider: string;
  sessionId: string | null;
  costUsd: number;
  turns: number;
  usage: TokenUsage;
}

/** Self-contained HTML transcript: inline styles, inline images, no scripts. */
export function transcriptHtml(blocks: TranscriptBlock[], meta: TranscriptExportMeta): string {
  const rows = blocks
    .map((b) => {
      if (b.kind === 'user') {
        const imgs = (b.images ?? [])
          .map((src) => `<img src="${esc(src)}" alt="attachment" />`)
          .join('');
        return `<section class="row user"><h2>You</h2>${imgs}<p>${esc(b.text ?? '')}</p></section>`;
      }
      if (b.kind === 'thinking') {
        return `<details class="row"><summary>thinking</summary><pre>${esc(b.text ?? '')}</pre></details>`;
      }
      if (b.kind === 'error') {
        return `<section class="row error"><p>${esc(b.text ?? '')}</p></section>`;
      }
      if (b.kind === 'tool') {
        const a = b.call?.args ?? {};
        const target =
          typeof a.path === 'string'
            ? a.path
            : typeof a.command === 'string'
              ? a.command
              : typeof a.pattern === 'string'
                ? a.pattern
                : '';
        const status = b.ok === false ? 'failed' : b.ok ? 'ok' : 'running';
        return `<section class="row tool"><code>${esc(b.call?.name ?? 'tool')}</code> <span>${esc(target)}</span> <span class="dim">${status}</span>${
          b.preview ? `<pre>${esc(b.preview)}</pre>` : ''
        }</section>`;
      }
      return `<section class="row">${esc(b.text ?? '')}</section>`;
    })
    .join('\n');

  const usage = meta.usage;
  const cost = meta.costUsd > 0 ? `$${meta.costUsd.toFixed(4)}` : '—';
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8" />
<meta name="viewport" content="width=device-width,initial-scale=1" />
<title>${esc(meta.title)}</title>
<style>
  :root { color-scheme: dark; }
  body { margin: 0; padding: 48px 20px; background: #0b0b0d; color: #eceae5;
    font: 15px/1.6 -apple-system, "Segoe UI", system-ui, sans-serif; }
  main { max-width: 760px; margin: 0 auto; }
  h1 { font-size: 22px; font-weight: 600; margin: 0 0 6px; }
  .meta { color: #8d8a82; font: 12px/1.6 ui-monospace, Consolas, monospace; margin: 0 0 28px; }
  .row { margin: 0 0 18px; }
  .row h2 { font: 600 11px/1 ui-monospace, monospace; letter-spacing: .12em;
    text-transform: uppercase; color: #8d8a82; margin: 0 0 6px; }
  .row p { margin: 0; white-space: pre-wrap; }
  .row.user { border-left: 2px solid #2b2d31; padding-left: 14px; }
  .row.error p { color: #d96a5c; }
  .row.tool { font: 12.5px/1.5 ui-monospace, monospace; color: #c3c0b8;
    border: 1px solid #1f2024; border-radius: 8px; padding: 8px 10px; }
  .row.tool code { color: #eceae5; }
  .dim { color: #6e6b64; }
  pre { margin: 8px 0 0; white-space: pre-wrap; word-break: break-word;
    font: 12px/1.5 ui-monospace, monospace; color: #a9a69e; }
  img { max-width: 100%; border-radius: 8px; margin: 6px 0; display: block; }
  details summary { cursor: pointer; color: #8d8a82; font: 12px ui-monospace, monospace; }
  footer { margin-top: 36px; padding-top: 14px; border-top: 1px solid #1f2024;
    color: #6e6b64; font: 12px ui-monospace, monospace; }
</style></head>
<body><main>
<h1>${esc(meta.title)}</h1>
<p class="meta">${esc(meta.provider)} / ${esc(meta.model)} · ${meta.turns} turns · ${usage.input} in / ${usage.output} out · ${cost}</p>
${rows}
<footer>exported from hate-agent${meta.sessionId ? ` · session ${esc(meta.sessionId)}` : ''}</footer>
</main></body></html>`;
}

export function transcriptJson(blocks: TranscriptBlock[], meta: TranscriptExportMeta): string {
  return JSON.stringify(
    {
      app: 'hate-agent',
      exportedAt: new Date().toISOString(),
      session: {
        id: meta.sessionId,
        title: meta.title,
        provider: meta.provider,
        model: meta.model,
        turns: meta.turns,
        costUsd: meta.costUsd,
        usage: meta.usage,
      },
      blocks: blocks.map((b) => ({
        kind: b.kind,
        text: b.text ?? null,
        images: b.images ?? null,
        tool: b.call ?? null,
        ok: b.ok ?? null,
        preview: b.preview ?? null,
        tokens: b.fullTokens ?? null,
        sentTokens: b.sentTokens ?? null,
      })),
    },
    null,
    2,
  );
}
