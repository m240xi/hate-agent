<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  // Inline permission bar above the composer — exact command/diff plus
  // Run / Run & remember / Deny. Never a modal, never confirm().
  const req = $derived(session.pendingPermission);

  const detail = $derived.by(() => {
    if (!req) return '';
    const args = req.call.args as Record<string, unknown>;
    return (
      req.detail ??
      (typeof args?.command === 'string'
        ? args.command
        : typeof args?.path === 'string'
          ? `${req.call.name} ${args.path}`
          : JSON.stringify(args).slice(0, 120))
    );
  });
</script>

{#if req}
  <div class="perm-bar rise" role="alertdialog" aria-label="Permission requested">
    <div class="perm-head">
      <Icon name="alert" size={13} />
      <span class="perm-title">{req.call.name} needs approval</span>
      <span class="perm-reason">{req.reason}</span>
    </div>
    <pre class="perm-detail mono">{detail}</pre>
    <div class="perm-actions">
      <button class="btn primary" onclick={() => session.respondPermission('allow')}>Run</button>
      <button class="btn" onclick={() => session.respondPermission('allow_session')}>
        Run &amp; remember
      </button>
      <button class="btn danger" onclick={() => session.respondPermission('deny')}>Deny</button>
    </div>
  </div>
{/if}

<style>
  .perm-bar {
    flex: none;
    min-width: 0;
    margin: 0 16px 8px;
    padding: 10px 12px;
    background: var(--bg-1);
    border: 1px solid var(--warn);
    border-radius: var(--radius);
  }
  .perm-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 8px;
    min-width: 0;
  }
  .perm-title {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--warn);
  }
  .perm-reason {
    font-size: 11px;
    color: var(--fg-3);
  }
  .perm-detail {
    margin: 8px 0;
    padding: 8px;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-size: 11.5px;
    color: var(--fg-1);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 140px;
    overflow: auto;
  }
  .perm-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .btn {
    padding: 4px 12px;
    font-size: 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-2);
    color: var(--fg-1);
  }
  .btn:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
    font-weight: 600;
  }
  .btn.primary:hover {
    filter: brightness(1.08);
  }
  .btn.danger {
    color: var(--err);
  }
  .btn.danger:hover {
    border-color: var(--err);
  }
</style>
