<script lang="ts">
  // Empty state — not a hero image. Five lines on how this works plus the
  // three most useful commands, and the Nous Portal connect flow.
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import type { NousLoginInfo } from '../ipc';

  let login: NousLoginInfo | null = $state(null);
  let loginError = $state<string | null>(null);
  let connected = $state(false);
  let busy = $state(false);
  let unlisten: UnlistenFn | null = null;

  onMount(async () => {
    try {
      const m = await import('../ipc');
      const s = await m.authStatus();
      connected = s.kind === 'oauth' || s.kind === 'api_key';
    } catch {
      /* not in Tauri — fixture mode */
    }
    try {
      const { listen } = await import('@tauri-apps/api/event');
      unlisten = await listen<{ type: string; ok: boolean; error?: string }>(
        'auth-event',
        (e) => {
          if (e.payload.type === 'nous_login') {
            busy = false;
            if (e.payload.ok) {
              connected = true;
              login = null;
            } else {
              loginError = e.payload.error ?? 'login failed';
            }
          }
        },
      );
    } catch {
      /* not in Tauri — fixture mode */
    }
  });

  onDestroy(() => unlisten?.());

  async function connect() {
    busy = true;
    loginError = null;
    try {
      const m = await import('../ipc');
      login = await m.authNousLogin();
    } catch (e) {
      busy = false;
      loginError = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<div class="empty">
  <div class="wordmark">hate-agent</div>
  <p class="line">
    A coding agent that spends fewer tokens per task — small prompt, lazy
    tools, shaped output, verified work.
  </p>
  {#if connected}
    <p class="connected">Connected to Nous Portal — your subscription is active.</p>
  {:else if login}
    <div class="login">
      <p class="login-line">
        Open <a href={login.verification_url} target="_blank" rel="noreferrer">{login.verification_url}</a>
        and enter code <code>{login.user_code}</code>
      </p>
      <button class="ghost" onclick={() => void import('../ipc').then((m) => m.authNousCancel())}>
        Cancel
      </button>
    </div>
  {:else}
    <button class="connect" onclick={connect} disabled={busy}>
      {busy ? 'Requesting code…' : 'Connect Nous Portal'}
    </button>
    {#if loginError}
      <p class="login-error">{loginError}</p>
    {/if}
  {/if}
  <ol class="how">
    <li>Describe a task in the composer below.</li>
    <li>The agent reads, edits, and runs checks — every tool call shows its token cost.</li>
    <li>It verifies before claiming done; you see the real check output.</li>
  </ol>
  <div class="cmds">
    <span class="cmd"><kbd>/</kbd> commands</span>
    <span class="cmd"><kbd>@</kbd> attach file</span>
    <span class="cmd"><kbd>esc</kbd> cancel turn</span>
  </div>
</div>

<style>
  .empty {
    max-width: 520px;
    margin: 12vh auto 0;
    padding: 0 24px;
    color: var(--fg-2);
  }
  .wordmark {
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--fg-0);
    margin-bottom: 10px;
  }
  .line {
    font-size: 13px;
    line-height: 1.6;
    margin: 0 0 16px;
  }
  .how {
    margin: 0 0 20px;
    padding-left: 20px;
    font-size: 12.5px;
    line-height: 1.7;
  }
  .how li {
    margin-bottom: 2px;
  }
  .cmds {
    display: flex;
    gap: 16px;
    font-size: 11.5px;
  }
  .connect {
    font: inherit;
    font-size: 12.5px;
    padding: 7px 14px;
    border-radius: 6px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--accent-fg, #fff);
    cursor: pointer;
    margin-bottom: 8px;
  }
  .connect:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .login {
    font-size: 12.5px;
    line-height: 1.6;
    margin-bottom: 8px;
  }
  .login a {
    color: var(--accent);
  }
  .login code {
    font-family: var(--font-mono);
    background: var(--bg-2);
    padding: 1px 6px;
    border-radius: 4px;
  }
  .ghost {
    font: inherit;
    font-size: 11.5px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-1);
    cursor: pointer;
  }
  .connected {
    font-size: 12.5px;
    color: var(--ok, #3fb950);
  }
  .login-error {
    font-size: 12px;
    color: var(--err, #f85149);
  }
  .cmd {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-3);
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    padding: 1px 5px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-2);
  }
</style>
