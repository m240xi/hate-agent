<script lang="ts">
  import { session } from '../store.svelte';
  import type { TranscriptBlock } from '../types';

  function base(p: string): string {
    return p.split(/[\\/]/).filter(Boolean).pop() ?? p;
  }

  function clip(s: string, n = 42): string {
    const one = s.replace(/\s+/g, ' ').trim();
    return one.length > n ? `${one.slice(0, n - 1)}…` : one;
  }

  function str(v: unknown): string {
    return typeof v === 'string' ? v : '';
  }

  function toolLabel(b: TranscriptBlock): string {
    const name = b.call?.name ?? '';
    const a = b.call?.args ?? {};
    const path = str(a.path) || str(a.file);
    if (name.startsWith('mcp__')) return `Using ${name.split('__')[1] ?? 'a connector'}`;
    switch (name) {
      case 'read_file':
      case 'read':
        return path ? `Reading ${base(path)}` : 'Reading a file';
      case 'list_dir':
        return path && path !== '.' ? `Looking through ${base(path)}` : 'Looking through the folder';
      case 'glob':
      case 'grep':
      case 'symbol':
      case 'find': {
        const q = str(a.pattern) || str(a.query) || str(a.name);
        return q ? `Searching for ${clip(q, 32)}` : 'Searching';
      }
      case 'write_file':
      case 'edit_file':
      case 'edit':
      case 'write':
      case 'apply_patch':
        return path ? `Editing ${base(path)}` : 'Editing files';
      case 'bash': {
        const cmd = str(a.command);
        return cmd ? `Running ${clip(cmd)}` : 'Running a command';
      }
      default:
        return 'Working';
    }
  }

  const label = $derived.by(() => {
    if (!session.running || session.pendingPermission) return null;
    const last = session.blocks.at(-1);
    if (!last || last.kind === 'user') return 'Thinking';
    if (last.kind === 'text' && last.streaming && last.text) return null;
    if (last.kind === 'thinking') return 'Thinking';
    if (last.kind === 'tool') return last.ok === undefined ? toolLabel(last) : 'Working';
    return 'Thinking';
  });
</script>

{#if label}
  <div class="live" role="status" aria-live="off">
    <span class="orb" aria-hidden="true"></span>
    <span class="shimmer">{label}…</span>
  </div>
{/if}

<style>
  .live {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 26px;
    font-size: 13.5px;
  }
  .orb {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex: none;
    background: var(--fg-1);
    animation: breathe 1.6s ease-in-out infinite;
  }
  .shimmer {
    color: var(--fg-3);
    background: linear-gradient(
      90deg,
      var(--fg-3) 0%,
      var(--fg-3) 35%,
      var(--fg-0) 50%,
      var(--fg-3) 65%,
      var(--fg-3) 100%
    );
    background-size: 220% 100%;
    background-clip: text;
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    animation: sweep 1.8s linear infinite;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  @keyframes breathe {
    50% {
      transform: scale(0.7);
      opacity: 0.45;
    }
  }
  @keyframes sweep {
    from {
      background-position: 110% 0;
    }
    to {
      background-position: -110% 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .shimmer {
      -webkit-text-fill-color: currentColor;
      color: var(--fg-2);
    }
  }
</style>
