<script lang="ts">
  import { session } from '../store.svelte';
  import Icon from './Icon.svelte';

  const steps = $derived(session.plan.length);
</script>

{#if session.planReady && !session.running}
  <div class="plan-bar rise" role="status" aria-label="Plan ready">
    <div class="plan-head">
      <Icon name="list" size={13} />
      <span class="plan-title">Plan ready</span>
      <span class="plan-meta">{steps} step{steps === 1 ? '' : 's'} · nothing has been changed</span>
    </div>
    <div class="plan-actions">
      <button class="btn primary" onclick={() => session.runPlan()}>Run plan</button>
      <button class="btn" onclick={() => session.revisePlan()} title="Stay in plan mode and reply with changes">
        Revise
      </button>
      <button class="btn danger" onclick={() => session.discardPlan()}>Discard</button>
    </div>
  </div>
{/if}

<style>
  .plan-bar {
    flex: none;
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
    margin: 0 16px;
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--accent-dim);
    border-radius: var(--radius);
  }
  .plan-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1 1 200px;
    min-width: 0;
    color: var(--accent);
  }
  .plan-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-0);
    flex: none;
  }
  .plan-meta {
    font-size: 11px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .plan-actions {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .btn {
    padding: 3px 10px;
    font-size: 11.5px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    color: var(--fg-1);
  }
  .btn:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
  }
  .btn.danger:hover {
    color: var(--err);
    border-color: var(--err);
  }
</style>
