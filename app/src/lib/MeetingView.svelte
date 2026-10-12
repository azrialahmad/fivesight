<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import AskBar from './AskBar.svelte';
  import Icon from './Icon.svelte';
  import Markdown from './Markdown.svelte';
  import Transcript from './Transcript.svelte';
  import { session } from './session.svelte';
  import { clock, dayLabel, timeOfDay } from './format';

  let now = $state(Date.now());
  let timer: ReturnType<typeof setInterval>;
  onMount(() => (timer = setInterval(() => (now = Date.now()), 500)));
  onDestroy(() => clearInterval(timer));

  const m = $derived(session.current!);
  const live = $derived(session.isRecordingCurrent);
  const elapsed = $derived(session.startedAt ? clock(now - session.startedAt) : '0:00');
  const canEnhance = $derived(session.segs.length > 0 || m.notes.trim().length > 0);
  const shownNotes = $derived(session.enhancing ? session.enhancedLive : m.enhanced);

  const sub = $derived(
    [dayLabel(m.startedAt), timeOfDay(m.startedAt), session.segs.length ? `${session.segs.length} lines of transcript` : '']
      .filter(Boolean)
      .join(' · ')
  );
</script>

<section class="relative flex h-full min-w-0 flex-col">
  <header class="flex items-center gap-3 whitespace-nowrap px-6 pb-3 pt-5">
    <div class="min-w-0">
      <input
        class="w-full min-w-0 max-w-md rounded-lg bg-transparent px-1 py-0.5 text-[22px] font-semibold tracking-tight outline-none hover:bg-paper focus:bg-paper"
        value={m.title}
        aria-label="Meeting title"
        onblur={(e) => session.rename(e.currentTarget.value)}
        onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
      />
      <p class="px-1 text-xs text-muted">{sub}</p>
    </div>

    {#if live}
      <span class="flex items-center gap-2 rounded-full bg-rec-soft px-3 py-1 text-xs font-semibold text-rec">
        <span class="rec-dot h-2 w-2 rounded-full bg-rec"></span>
        Recording {elapsed}
      </span>
    {:else if session.busy}
      <span class="text-xs text-muted">Loading models…</span>
    {/if}

    <div class="ml-auto flex items-center gap-2">
      <button
        class="flex items-center gap-2 rounded-full border px-4 py-2 text-[13px] font-medium {session.drawer ? 'border-ink bg-ink text-white' : 'border-line hover:border-ink'}"
        onclick={() => (session.drawer = !session.drawer)}
        aria-pressed={session.drawer}
      >
        <Icon name="lines" size={16} /> Transcript
      </button>
      {#if live}
        <button class="flex items-center gap-2 rounded-full bg-rec px-5 py-2 text-[13px] font-semibold text-white disabled:opacity-60" disabled={session.busy} onclick={() => session.stopRecording()}>
          <Icon name="stop" size={16} /> Stop
        </button>
      {:else if !session.recording}
        <button class="flex items-center gap-2 rounded-full bg-ink px-5 py-2 text-[13px] font-semibold text-white disabled:opacity-60" disabled={session.busy} onclick={() => session.startRecording()}>
          <span class="h-2 w-2 rounded-full bg-lime"></span>
          {session.busy ? 'Starting…' : m.endedAt ? 'Record again' : 'Start recording'}
        </button>
      {/if}
    </div>
  </header>

  {#if session.error}
    <div class="mx-6 mb-2 rounded-xl bg-rec-soft px-4 py-2.5 text-sm text-rec" role="alert">{session.error}</div>
  {/if}

  <div class="grid min-h-[9rem] flex-1 grid-cols-2 border-t border-line">
    <div class="flex min-h-0 flex-col border-r border-line">
      <div class="px-6 pb-1 pt-4 text-[11px] font-semibold tracking-wider text-faint">YOUR NOTES</div>
      <textarea
        class="min-h-0 w-full flex-1 resize-none bg-transparent px-6 pb-4 text-[15px] leading-relaxed outline-none placeholder:text-faint"
        placeholder="Jot quick notes while you talk. fivesight fills in the rest."
        value={m.notes}
        oninput={(e) => session.setNotes(e.currentTarget.value)}
        aria-label="Your notes"
      ></textarea>
    </div>

    <div class="flex min-h-0 flex-col bg-paper/60">
      <div class="flex items-center px-6 pb-1 pt-3">
        <span class="text-[11px] font-semibold tracking-wider text-faint">AI NOTES</span>
        <button
          class="ml-auto flex items-center gap-1.5 rounded-full bg-lime px-3.5 py-1.5 text-[13px] font-semibold text-ink transition-colors hover:bg-lime-deep disabled:bg-soft disabled:text-faint"
          disabled={!canEnhance || session.enhancing}
          title={canEnhance ? '' : 'Take some notes or record first'}
          onclick={() => session.enhance()}
        >
          <Icon name="spark" size={15} />
          {session.enhancing ? 'Writing…' : m.enhanced ? 'Enhance again' : 'Enhance notes'}
        </button>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto px-6 pb-4 pt-2">
        {#if session.enhancing && !session.enhancedLive}
          <div class="space-y-3 pt-2" aria-label="Writing notes">
            <div class="shimmer h-4 w-2/3 rounded-full"></div>
            <div class="shimmer h-3 w-full rounded-full"></div>
            <div class="shimmer h-3 w-11/12 rounded-full"></div>
            <div class="shimmer h-3 w-3/4 rounded-full"></div>
          </div>
        {:else if shownNotes}
          <Markdown text={shownNotes} />
        {:else}
          <div class="flex h-full flex-col items-center justify-center text-center">
            <span class="grid h-12 w-12 place-items-center rounded-2xl bg-lime text-ink"><Icon name="spark" size={24} /></span>
            <p class="mt-4 font-semibold">Your notes, expanded</p>
            <p class="mt-1 max-w-[16rem] text-sm leading-relaxed text-muted">
              Take a few quick notes while you talk. When you're ready, press Enhance and fivesight writes it up from what was said.
            </p>
          </div>
        {/if}
      </div>
    </div>
  </div>

  <AskBar />

  {#if session.drawer}
    <aside class="slide-in absolute inset-y-0 right-0 z-10 flex w-[22rem] flex-col border-l border-line bg-card shadow-[-12px_0_32px_-16px_rgba(0,0,0,0.18)]">
      <div class="flex items-center border-b border-line px-5 py-3.5">
        <h2 class="font-semibold">Transcript</h2>
        <button class="ml-auto grid h-8 w-8 place-items-center rounded-full text-muted hover:bg-soft" aria-label="Close transcript" onclick={() => (session.drawer = false)}>
          <Icon name="close" size={18} />
        </button>
      </div>
      <div class="min-h-0 flex-1">
        {#if session.segs.length === 0}
          <p class="p-6 text-sm leading-relaxed text-muted">{live ? 'Listening… lines appear here as people talk.' : 'No transcript for this meeting yet.'}</p>
        {:else}
          <Transcript compact />
        {/if}
      </div>
    </aside>
  {/if}
</section>
