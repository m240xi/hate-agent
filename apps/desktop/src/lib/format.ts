// Shared display formatting for token counts, usage records, and
// clipboard writes. Pure functions — no store access.

import type { PlanStep, TokenUsage, TranscriptBlock, TurnSummary } from './types';

/** Compact token count: 950, 12.3k, 1.2M. */
export function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

/** One provider request, compact: `in 12.3k (9.1k cached) · out 412`.
 *  `input` already includes the cached portion (core's convention). */
export function usageLabel(u: TokenUsage, withThinking = true): string {
  const cached = u.cached_input > 0 ? ` (${fmtTok(u.cached_input)} cached)` : '';
  // Thinking headers already show the reasoning count — don't say it twice.
  const think = withThinking && u.thinking > 0 ? ` · think ${fmtTok(u.thinking)}` : '';
  return `in ${fmtTok(u.input)}${cached} · out ${fmtTok(u.output)}${think}`;
}

/** Exact numbers for the hover tooltip. */
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

/** Whole-turn rollup shown on the user's message once the turn ends. */
export function turnLabel(t: TurnSummary): string {
  const parts = [usageLabel(t.usage)];
  if (t.toolCalls > 0) parts.push(`${t.toolCalls} tool${t.toolCalls === 1 ? '' : 's'}`);
  parts.push(fmtElapsed(t.elapsedMs));
  if (t.costUsd !== null && t.costUsd > 0) parts.push(`$${t.costUsd.toFixed(4)}`);
  return parts.join(' · ');
}

/** Copy text to the clipboard; falls back to a hidden textarea where
 *  the async clipboard API is unavailable. Resolves to success. */
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

/** Pull plan steps out of a plan-mode reply: numbered lines when the
 *  model numbered them, otherwise top-level bullets. */
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

/** The transcript as Markdown, for `/export`. Tool calls collapse to
 *  one line each; thinking is omitted. */
export function transcriptMarkdown(blocks: TranscriptBlock[]): string {
  let out = '';
  let lastWasTool = false;
  const push = (chunk: string, isTool: boolean): void => {
    // Consecutive tool lines form one list; everything else is a paragraph.
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
