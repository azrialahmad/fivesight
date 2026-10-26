<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import AskBar from './AskBar.svelte';
  import Icon from './Icon.svelte';
  import Markdown from './Markdown.svelte';
  import NotesEditor from './NotesEditor.svelte';
  import Transcript from './Transcript.svelte';
  import { session } from './session.svelte';
  import { clock, dayLabel, timeOfDay } from './format';
  import { pastedMarkdown } from './markdown';
  import type { MeetingDoc, NotesMode } from './api';

  let now = $state(Date.now());
  let timer: ReturnType<typeof setInterval>;
  onMount(() => (timer = setInterval(() => (now = Date.now()), 500)));
  onDestroy(() => clearInterval(timer));

  const languages: [string, string][] = [
    ['auto', 'Indonesian + English'],
    ['id', 'Indonesian only'],
    ['en', 'English only']
  ];

  const m = $derived(session.current!);
  const live = $derived(session.isRecordingCurrent);
  const elapsed = $derived(session.startedAt ? clock(now - session.startedAt) : '0:00');
  const mode = $derived(session.settings.notesMode);
  const auto = $derived(mode === 'live' && live);
  // When the AI takes the notes there's one page: it writes, you correct and add to it.
  const onePage = $derived(mode === 'live');
  const canEnhance = $derived(session.segs.length > 0 || m.notes.trim().length > 0 || (onePage && m.enhanced.trim().length > 0));
  // An automatic pass keeps showing the last notes until the new ones are complete, so they
  // don't blank out and retype every minute. A manual pass (or the very first) streams in;
  // on the one page you keep editing during automatic passes, so only a manual one streams.
  const streaming = $derived(session.enhancing && (!session.enhancingAuto || (!onePage && !m.enhanced)));
  const shownNotes = $derived(streaming ? session.enhancedLive : m.enhanced);
  const enhanceLabel = $derived(streaming ? 'Writing…' : onePage ? 'Update notes' : m.enhanced ? 'Enhance again' : 'Enhance notes');
  const modes: [NotesMode, string, string][] = [
    ['live', 'AI takes notes', 'The AI keeps the notes as the meeting goes, a small update every couple of minutes. You edit and correct them on the same page.'],
    ['end', 'I take notes, AI enhances', 'You take notes. When you stop recording, the AI fills them in once from the whole meeting.'],
    ['manual', 'Only when I press Enhance', 'Nothing is sent to the AI until you press Enhance.']
  ];

  let pageLink = $state('');
  let addingPage = $state(false);
  async function attach() {
    if (await session.attachPage(pageLink)) {
      pageLink = '';
      addingPage = false;
    }
  }
  const kb = (n: number) => (n < 1024 ? `${n} B` : `${Math.round(n / 1024)} KB`);

  // Earlier meetings this one continues: the ones with the same name first, then the newest.
  let picking = $state(false);
  let pickQuery = $state('');
  const attachedMeetings = $derived(new Set(session.docs.filter((d) => d.source === 'meeting').map((d) => d.ref)));
  const sameName = (title: string) => title !== 'Untitled meeting' && title.trim().toLowerCase() === m.title.trim().toLowerCase();
  const candidates = $derived(
    session.meetings
      .filter((x) => x.id !== m.id && x.startedAt <= m.startedAt + 60_000 && !attachedMeetings.has(String(x.id)))
      .filter((x) => x.title.toLowerCase().includes(pickQuery.trim().toLowerCase()))
      .sort((a, b) => Number(sameName(b.title)) - Number(sameName(a.title)) || b.startedAt - a.startedAt)
      .slice(0, 8)
  );
  async function pickMeeting(id: number) {
    if (await session.attachMeeting(id)) closePicker();
  }
  function closePicker() {
    picking = false;
    pickQuery = '';
    session.attachError = null;
  }
  const earlier = (d: MeetingDoc) => session.meetings.find((x) => String(x.id) === d.ref);
  function docTitle(d: MeetingDoc): string {
    if (d.source === 'meeting') return 'Earlier meeting. The AI notes and Ask read its notes, to follow up on what was decided and still open.';
    const comments =
      d.comments == null ? "Its comments couldn't be read." : 'With its comments (inline, footer and resolved), where decisions often end up.';
    return `Confluence page ${d.ref}, read in full. ${comments} The AI notes and Ask use it.`;
  }

  /** Pasting formatted text into your notes keeps it as Markdown (lists, headings, bold). */
  function pasteNotes(e: ClipboardEvent) {
    const md = pastedMarkdown(e.clipboardData);
    if (md === null) return;
    e.preventDefault();
    // Typed in like a paste: it can be undone, and the input event saves it.
    document.execCommand('insertText', false, md);
  }

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
      <label
        class="relative flex items-center rounded-full border border-line text-[13px] font-medium focus-within:border-ink {live ? 'opacity-60' : 'hover:border-ink'}"
        title={live ? 'Stop recording to change the language' : 'What language is this meeting in? Applies the next time you record.'}
      >
        <span class="sr-only">Meeting language</span>
        <select
          class="appearance-none rounded-full bg-transparent py-2 pl-4 pr-9 outline-none disabled:cursor-not-allowed"
          value={m.language}
          disabled={live}
          onchange={(e) => session.setMeetingLanguage(e.currentTarget.value)}
        >
          {#each languages as [value, label]}
            <option {value}>{label}</option>
          {/each}
        </select>
        <span class="pointer-events-none absolute right-3 rotate-90 text-faint"><Icon name="chevron" size={16} /></span>
      </label>
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

  <div class="flex flex-wrap items-center gap-2 px-6 pb-3 text-[13px]">
    {#each session.docs as d (d.id)}
      <span class="flex max-w-xs items-center gap-1.5 rounded-full border border-line bg-paper py-1 pl-3 pr-1" title={docTitle(d)}>
        <Icon name={d.source === 'meeting' ? 'history' : 'book'} size={14} />
        <span class="truncate font-medium">{d.title}</span>
        {#if d.source === 'meeting'}
          {@const e = earlier(d)}
          <span class="shrink-0 text-xs text-faint">{e ? dayLabel(e.startedAt) : 'deleted'}</span>
        {:else}
          <span class="shrink-0 text-xs text-faint">{kb(d.bytes)}{d.comments ? ` · ${d.comments} comment${d.comments === 1 ? '' : 's'}` : ''}</span>
        {/if}
        <button class="grid h-6 w-6 shrink-0 place-items-center rounded-full text-muted hover:bg-soft" aria-label="Remove {d.title}" onclick={() => session.detachPage(d.id)}>
          <Icon name="close" size={13} />
        </button>
      </span>
    {/each}
    {#if addingPage}
      <form class="flex min-w-0 flex-1 items-center gap-2" onsubmit={(e) => (e.preventDefault(), attach())}>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="min-w-0 max-w-md flex-1 rounded-full border border-line bg-card px-3.5 py-1.5 outline-none focus:border-ink"
          placeholder="Paste a Confluence page link"
          aria-label="Confluence page link"
          bind:value={pageLink}
          autofocus
          disabled={session.attaching}
          onkeydown={(e) => e.key === 'Escape' && ((addingPage = false), (session.attachError = null))}
        />
        <button class="rounded-full bg-ink px-3.5 py-1.5 font-semibold text-white disabled:opacity-60" disabled={session.attaching || !pageLink.trim()}>
          {session.attaching ? 'Reading page…' : 'Add'}
        </button>
        <button type="button" class="rounded-full px-2 py-1.5 text-muted hover:text-ink" onclick={() => ((addingPage = false), (session.attachError = null))}>Cancel</button>
      </form>
    {:else}
      <button
        class="flex items-center gap-1.5 rounded-full border border-dashed border-line px-3 py-1 text-muted hover:border-ink hover:text-ink"
        title="Read a whole Confluence page (a PRD, a spec) and its comments so the AI notes and Ask know it during the meeting"
        onclick={() => ((addingPage = true), (picking = false))}
      >
        <Icon name="plus" size={14} /> Confluence page
      </button>
      <div class="relative">
        <button
          class="flex items-center gap-1.5 rounded-full border border-dashed px-3 py-1 hover:border-ink hover:text-ink {picking ? 'border-ink text-ink' : 'border-line text-muted'}"
          title="This meeting continues an earlier one: the AI notes and Ask read its notes to follow up on it"
          aria-expanded={picking}
          onclick={() => (picking ? closePicker() : ((picking = true), (session.attachError = null)))}
        >
          <Icon name="plus" size={14} /> Earlier meeting
        </button>
        {#if picking}
          <button class="fixed inset-0 z-20 cursor-default" aria-label="Close" tabindex="-1" onclick={closePicker}></button>
          <div class="rise absolute left-0 top-full z-30 mt-1.5 w-80 rounded-2xl border border-line bg-card p-2 shadow-[0_12px_32px_-12px_rgba(0,0,0,0.25)]">
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="w-full rounded-full border border-line bg-paper px-3.5 py-1.5 outline-none focus:border-ink"
              placeholder="Which meeting does this continue?"
              aria-label="Find an earlier meeting"
              bind:value={pickQuery}
              autofocus
              onkeydown={(e) => e.key === 'Escape' && closePicker()}
            />
            <ul class="mt-1.5 max-h-72 overflow-y-auto">
              {#each candidates as c (c.id)}
                <li>
                  <button
                    class="flex w-full items-center gap-2 rounded-xl px-3 py-2 text-left hover:bg-soft disabled:opacity-60"
                    disabled={session.attaching}
                    onclick={() => pickMeeting(c.id)}
                  >
                    <span class="min-w-0 flex-1 truncate font-medium">{c.title}</span>
                    {#if sameName(c.title)}<span class="shrink-0 rounded-full bg-lime px-2 py-0.5 text-[10px] font-semibold text-ink">Same name</span>{/if}
                    <span class="shrink-0 text-xs text-faint">{dayLabel(c.startedAt)}</span>
                  </button>
                </li>
              {:else}
                <li class="px-3 py-2 text-xs text-muted">{pickQuery.trim() ? 'No earlier meeting by that name.' : 'No earlier meetings yet.'}</li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>
    {/if}
    {#if session.attachError}
      <span class="basis-full text-xs text-rec" role="alert">{session.attachError}</span>
    {/if}
  </div>

  {#snippet toolbar(heading: string)}
      <div class="flex items-center whitespace-nowrap px-6 pb-1 pt-3">
        <span class="shrink-0 text-[11px] font-semibold tracking-wider text-faint">{heading}</span>
        <label class="relative ml-2 flex min-w-0 items-center text-xs text-muted hover:text-ink" title={modes.find(([v]) => v === mode)?.[2]}>
          <span class="sr-only">Who writes the notes</span>
          <select class="min-w-0 appearance-none truncate rounded-full bg-transparent py-1 pl-2 pr-6 outline-none focus:bg-card" value={mode} onchange={(e) => session.setNotesMode(e.currentTarget.value as NotesMode)}>
            {#each modes as [value, label]}
              <option {value}>{label}</option>
            {/each}
          </select>
          <span class="pointer-events-none absolute right-1 rotate-90 text-faint"><Icon name="chevron" size={13} /></span>
        </label>
        {#if session.enhancing && session.enhancingAuto}
          <span class="ml-3 flex items-center gap-1.5 text-xs text-muted" role="status">
            <span class="rec-dot h-1.5 w-1.5 rounded-full bg-lime-deep"></span>Updating…
          </span>
        {:else if auto && session.autoError}
          <span class="ml-3 min-w-0 truncate text-xs text-rec" title={session.autoError}>Couldn't update; trying again soon</span>
        {:else if auto}
          <span class="ml-1 flex items-center gap-1.5 text-xs text-muted" title="Updates when you pause typing and as the meeting goes on"><span class="h-1.5 w-1.5 rounded-full bg-lime-deep"></span>Live</span>
        {/if}
        <button
          class="ml-auto flex shrink-0 items-center gap-1.5 rounded-full bg-lime px-3.5 py-1.5 text-[13px] font-semibold text-ink transition-colors hover:bg-lime-deep disabled:bg-soft disabled:text-faint"
          disabled={!canEnhance || session.enhancing}
          title={!canEnhance ? 'Take some notes or record first' : onePage ? 'Catch the notes up with the meeting now. Your edits stay as you made them.' : ''}
          onclick={() => session.enhance()}
        >
          <Icon name="spark" size={15} />
          {enhanceLabel}
        </button>
      </div>
  {/snippet}

  {#snippet writing()}
    <div class="space-y-3 pt-2" aria-label="Writing notes">
      <div class="shimmer h-4 w-2/3 rounded-full"></div>
      <div class="shimmer h-3 w-full rounded-full"></div>
      <div class="shimmer h-3 w-11/12 rounded-full"></div>
      <div class="shimmer h-3 w-3/4 rounded-full"></div>
    </div>
  {/snippet}

  {#if onePage}
    <div class="flex min-h-[9rem] flex-1 flex-col border-t border-line">
      {@render toolbar('NOTES')}
      <div class="min-h-0 flex-1 overflow-y-auto px-6 pb-4 pt-2">
        <div class="mx-auto max-w-3xl">
          {#if streaming}
            {#if session.enhancedLive}<Markdown text={session.enhancedLive} />{:else}{@render writing()}{/if}
          {:else}
            <NotesEditor
              text={session.liveDoc}
              onedit={(md) => session.setAiNotes(md)}
              label="Meeting notes"
              placeholder="While you record, fivesight writes the notes here as the meeting goes. Fix anything it gets wrong and add your own lines: it keeps your edits and builds on them."
            />
          {/if}
        </div>
      </div>
    </div>
  {:else}
  <div class="grid min-h-[9rem] flex-1 grid-cols-2 border-t border-line">
    <div class="flex min-h-0 flex-col border-r border-line">
      <div class="px-6 pb-1 pt-4 text-[11px] font-semibold tracking-wider text-faint">YOUR NOTES</div>
      <textarea
        class="min-h-0 w-full flex-1 resize-none bg-transparent px-6 pb-4 text-[15px] leading-relaxed outline-none placeholder:text-faint"
        placeholder="Jot quick notes while you talk. fivesight fills in the rest."
        value={m.notes}
        oninput={(e) => session.setNotes(e.currentTarget.value)}
        onpaste={pasteNotes}
        aria-label="Your notes"
      ></textarea>
    </div>

    <div class="flex min-h-0 flex-col bg-paper/60">
      {@render toolbar('AI NOTES')}
      <div class="min-h-0 flex-1 overflow-y-auto px-6 pb-4 pt-2">
        {#if streaming && !session.enhancedLive}
          {@render writing()}
        {:else if shownNotes}
          <Markdown text={shownNotes} mine={m.notes} />
        {:else}
          <div class="flex h-full flex-col items-center justify-center text-center">
            <span class="grid h-12 w-12 place-items-center rounded-2xl bg-lime text-ink"><Icon name="spark" size={24} /></span>
            <p class="mt-4 font-semibold">Your notes, expanded</p>
            <p class="mt-1 max-w-[16rem] text-sm leading-relaxed text-muted">
              {#if mode === 'end'}
                Take a few quick notes while you talk. When you stop recording, fivesight fills them in from what was said.
              {:else}
                Take a few quick notes while you talk. When you're ready, press Enhance and fivesight writes it up from what was said.
              {/if}
            </p>
          </div>
        {/if}
      </div>
    </div>
  </div>
  {/if}

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
