<script lang="ts">
  import { onMount } from 'svelte';
  import Transcript from './lib/Transcript.svelte';
  import Icon from './lib/Icon.svelte';
  import { session, inTauri } from './lib/session.svelte';

  onMount(() => session.init());

  async function closePanel() {
    if (!inTauri) return;
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().hide();
  }
</script>

<div class="flex h-full flex-col overflow-hidden rounded-2xl border border-line bg-card">
  <header data-tauri-drag-region class="flex select-none items-center gap-2 px-4 py-2.5">
    <span data-tauri-drag-region class="flex items-center gap-2 text-sm font-medium">
      <span class="grid h-6 w-6 place-items-center rounded-lg bg-blue text-white"><Icon name="spark" size={14} /></span>
      fivesight
      {#if session.recording}<span class="rec-dot ml-1 h-2 w-2 rounded-full bg-red" title="Recording"></span>{/if}
    </span>
    <div class="ml-auto flex items-center gap-1">
      {#if session.recording}
        <button class="flex items-center gap-1.5 rounded-full bg-red px-3 py-1 text-xs font-medium text-white" onclick={() => session.stop()}>
          <Icon name="stop" size={14} /> Stop
        </button>
      {:else}
        <button
          class="flex items-center gap-1.5 rounded-full bg-blue px-3 py-1 text-xs font-medium text-white disabled:opacity-60"
          disabled={session.busy}
          onclick={() => session.start()}
        >
          <Icon name="mic" size={14} />
          {session.busy ? 'Loading…' : 'Start'}
        </button>
      {/if}
      <button class="grid h-7 w-7 place-items-center rounded-full text-muted hover:bg-hover" aria-label="Hide panel" onclick={closePanel}>
        <Icon name="close" size={16} />
      </button>
    </div>
  </header>

  {#if session.error}
    <div class="mx-3 mb-2 rounded-xl bg-red-soft px-3 py-2 text-xs text-red" role="alert">{session.error}</div>
  {/if}

  <div class="min-h-0 flex-1 border-t border-line">
    {#if session.segs.length === 0}
      <p class="p-6 text-center text-sm text-muted">{session.recording ? 'Listening…' : 'Press Start to transcribe.'}</p>
    {:else}
      <Transcript compact limit={12} />
    {/if}
  </div>
</div>
