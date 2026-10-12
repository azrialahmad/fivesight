<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import Transcript from './lib/Transcript.svelte';
  import Icon from './lib/Icon.svelte';
  import { session, inTauri } from './lib/session.svelte';

  let view = $state<'meeting' | 'settings'>('meeting');
  let now = $state(Date.now());
  let timer: ReturnType<typeof setInterval>;

  onMount(() => {
    session.init();
    timer = setInterval(() => (now = Date.now()), 500);
  });
  onDestroy(() => clearInterval(timer));

  const elapsed = $derived.by(() => {
    if (!session.startedAt) return '0:00';
    const s = Math.max(0, Math.floor((now - session.startedAt) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  });

  const languages: [string, string][] = [
    ['auto', 'Auto-detect (Indonesian + English)'],
    ['id', 'Indonesian'],
    ['en', 'English']
  ];

  const navBase = 'flex items-center gap-3 rounded-full px-4 py-2.5 text-left text-sm transition-colors';
</script>

<div class="flex h-full">
  <aside class="flex w-60 shrink-0 flex-col px-3 py-4">
    <div class="flex items-center gap-2.5 px-3 pb-5 pt-1">
      <span class="grid h-8 w-8 place-items-center rounded-xl bg-blue text-white"><Icon name="spark" size={18} /></span>
      <span class="text-[19px] font-medium tracking-tight">fivesight</span>
    </div>

    <button
      class="mb-4 flex items-center gap-3 self-start rounded-2xl bg-blue-soft px-5 py-3.5 font-medium text-blue shadow-sm transition-shadow hover:shadow-md disabled:opacity-60"
      disabled={session.busy || session.recording}
      onclick={() => {
        view = 'meeting';
        session.start();
      }}
    >
      <Icon name="mic" /> New meeting
    </button>

    <nav class="flex flex-col gap-0.5" aria-label="Main">
      <button
        class="{navBase} {view === 'meeting' ? 'bg-blue-soft font-medium text-blue' : 'hover:bg-hover'}"
        onclick={() => (view = 'meeting')}
        aria-current={view === 'meeting' ? 'page' : undefined}
      >
        <Icon name="mic" size={20} /> Live
      </button>
      <button class="{navBase} cursor-not-allowed text-muted opacity-50" disabled title="Coming soon">
        <Icon name="history" size={20} /> History
      </button>
      <button
        class="{navBase} {view === 'settings' ? 'bg-blue-soft font-medium text-blue' : 'hover:bg-hover'}"
        onclick={() => (view = 'settings')}
        aria-current={view === 'settings' ? 'page' : undefined}
      >
        <Icon name="settings" size={20} /> Settings
      </button>
    </nav>

    <p class="mt-auto px-3 text-xs leading-relaxed text-muted">
      Runs on this PC. Tell everyone in the call you're transcribing.
    </p>
  </aside>

  <main class="my-3 mr-3 flex min-w-0 flex-1 flex-col overflow-hidden rounded-3xl border border-line bg-card">
    {#if view === 'meeting'}
      <header class="flex items-center gap-3 whitespace-nowrap px-8 pb-3 pt-5">
        <h1 class="text-[22px] font-normal tracking-tight">Live meeting</h1>
        {#if session.recording}
          <span class="flex items-center gap-2 rounded-full bg-red-soft px-3 py-1 text-xs font-medium text-red">
            <span class="rec-dot h-2 w-2 rounded-full bg-red"></span>
            Recording {elapsed}
          </span>
        {:else if session.busy}
          <span class="text-xs text-muted">Loading models…</span>
        {/if}

        <div class="ml-auto flex items-center gap-2">
          {#if session.segs.length > 0 && !session.recording}
            <button class="flex items-center gap-2 rounded-full px-4 py-2 text-muted hover:bg-hover" onclick={() => session.clear()}>
              <Icon name="delete" size={18} /> Clear
            </button>
          {/if}
          {#if inTauri}
            <button class="flex items-center gap-2 rounded-full border border-line px-4 py-2 hover:bg-hover" onclick={() => session.openLivePanel()}>
              <Icon name="pip" size={18} /> Mini panel
            </button>
          {/if}
          {#if session.recording}
            <button
              class="flex items-center gap-2 rounded-full bg-red px-5 py-2 font-medium text-white transition-colors hover:brightness-95 disabled:opacity-60"
              disabled={session.busy}
              onclick={() => session.stop()}
            >
              <Icon name="stop" size={18} /> Stop
            </button>
          {:else}
            <button
              class="flex items-center gap-2 rounded-full bg-blue px-5 py-2 font-medium text-white transition-colors hover:bg-blue-hover disabled:opacity-60"
              disabled={session.busy}
              onclick={() => session.start()}
            >
              <Icon name="mic" size={18} />
              {session.busy ? 'Starting…' : 'Start recording'}
            </button>
          {/if}
        </div>
      </header>

      {#if session.error}
        <div class="mx-8 mb-2 rounded-2xl bg-red-soft px-4 py-3 text-sm text-red" role="alert">{session.error}</div>
      {/if}

      <div class="min-h-0 flex-1">
        {#if session.segs.length === 0}
          <div class="flex h-full items-center justify-center px-8 text-center">
            <div class="max-w-sm">
              <span class="mx-auto grid h-20 w-20 place-items-center rounded-full bg-blue-soft text-blue">
                <Icon name="mic" size={36} />
              </span>
              <p class="mt-6 text-xl">{session.recording ? 'Listening…' : 'Ready when you are'}</p>
              <p class="mt-2 leading-relaxed text-muted">
                {session.recording
                  ? 'Transcript lines appear here as people talk.'
                  : 'Start recording to transcribe your mic and the other side of the call. Nothing leaves this PC.'}
              </p>
              {#if !inTauri}
                <p class="mt-4 text-xs text-muted">Browser preview: using a scripted demo conversation.</p>
              {/if}
            </div>
          </div>
        {:else}
          <Transcript />
        {/if}
      </div>
    {:else}
      <header class="px-8 pb-3 pt-5">
        <h1 class="text-[22px] font-normal tracking-tight">Settings</h1>
      </header>
      <div class="w-full max-w-xl space-y-4 overflow-y-auto px-8 pb-8">
        <section class="rounded-2xl border border-line p-5">
          <label class="block font-medium" for="lang">Spoken language</label>
          <p class="mt-1 text-sm text-muted">Pinning one language is more accurate than auto-detect.</p>
          <select
            id="lang"
            class="mt-3 w-full rounded-xl border border-line bg-card px-3 py-2.5"
            value={session.language}
            onchange={(e) => session.setLanguage(e.currentTarget.value)}
          >
            {#each languages as [value, label]}
              <option {value}>{label}</option>
            {/each}
          </select>
        </section>
        <section class="rounded-2xl border border-line p-5">
          <h2 class="font-medium">Models</h2>
          <p class="mt-1 break-all text-sm text-muted">
            {session.modelsDir ??
              (inTauri
                ? 'Not found. Put ggml-large-v3-turbo.bin and silero_vad.onnx in %LOCALAPPDATA%\\fivesight\\models.'
                : 'Not available in browser preview.')}
          </p>
        </section>
      </div>
    {/if}
  </main>
</div>
