<script lang="ts">
  import Icon from './Icon.svelte';
  import { session } from './session.svelte';
  import { dayLabel, durationLabel, timeOfDay } from './format';

  const groups = $derived.by(() => {
    const out: { label: string; items: typeof session.meetings }[] = [];
    for (const m of session.meetings) {
      const label = dayLabel(m.startedAt);
      const last = out[out.length - 1];
      if (last?.label === label) last.items.push(m);
      else out.push({ label, items: [m] });
    }
    return out;
  });

  /** FTS snippets mark hits as [word]; render those as highlighter strokes. */
  function pieces(snippet: string): { text: string; hit: boolean }[] {
    return snippet.split(/(\[[^\]]+\])/).filter(Boolean).map((p) => (p.startsWith('[') && p.endsWith(']') ? { text: p.slice(1, -1), hit: true } : { text: p, hit: false }));
  }

  const searching = $derived(session.query.trim().length >= 2);
</script>

<div class="h-full overflow-y-auto">
  <div class="mx-auto max-w-2xl px-8 pb-10 pt-8">
    <div class="relative">
      <span class="pointer-events-none absolute left-4 top-1/2 -translate-y-1/2 text-faint"><Icon name="search" size={18} /></span>
      <input
        class="w-full rounded-full border border-line bg-paper py-2.5 pl-11 pr-4 text-sm outline-none placeholder:text-faint focus:border-ink"
        placeholder="Search everything that was said"
        aria-label="Search meetings"
        value={session.query}
        oninput={(e) => session.runSearch(e.currentTarget.value)}
      />
    </div>

    {#if searching}
      <h2 class="mb-2 mt-7 text-[11px] font-semibold tracking-wider text-faint">{session.hits.length} RESULTS</h2>
      {#if session.hits.length === 0}
        <p class="text-sm text-muted">Nothing matches "{session.query}".</p>
      {/if}
      <ul class="divide-y divide-line">
        {#each session.hits as h}
          <li>
            <button class="block w-full rounded-xl px-3 py-3 text-left hover:bg-paper" onclick={() => session.openMeeting(h.meetingId)}>
              <div class="text-xs font-semibold">{h.meetingTitle}</div>
              <p class="mt-0.5 text-sm text-muted">{#each pieces(h.snippet) as p}{#if p.hit}<span class="hl text-ink">{p.text}</span>{:else}{p.text}{/if}{/each}</p>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <div class="mt-7 rounded-3xl bg-lime p-7">
        <h1 class="text-2xl font-semibold tracking-tight">Ready for your next meeting?</h1>
        <p class="mt-1.5 max-w-sm text-[15px] leading-relaxed">Jot a few notes while fivesight listens. It writes the rest, and answers questions after.</p>
        <div class="mt-5 flex flex-wrap gap-2">
          <button
            class="flex items-center gap-2 rounded-full bg-ink px-5 py-2.5 text-sm font-semibold text-white disabled:opacity-60"
            disabled={session.busy || session.recording}
            onclick={async () => {
              await session.newMeeting();
              await session.startRecording();
            }}
          >
            <span class="h-2 w-2 rounded-full bg-lime"></span> Start recording
          </button>
          <button class="rounded-full border border-ink/25 px-5 py-2.5 text-sm font-medium hover:border-ink" onclick={() => session.newMeeting()}>Just take notes</button>
        </div>
      </div>

      {#if session.meetings.length === 0}
        <p class="mt-10 text-center text-sm text-muted">Your meetings will show up here.</p>
      {/if}

      {#each groups as g}
        <h2 class="mb-1 mt-8 text-[11px] font-semibold tracking-wider text-faint">{g.label.toUpperCase()}</h2>
        <ul class="divide-y divide-line">
          {#each g.items as m (m.id)}
            <li class="group flex items-center rounded-xl hover:bg-paper">
              <button class="flex min-w-0 flex-1 items-center gap-4 px-3 py-3 text-left" onclick={() => session.openMeeting(m.id)}>
                <span class="w-16 shrink-0 text-xs tabular-nums text-muted">{timeOfDay(m.startedAt)}</span>
                <span class="min-w-0 flex-1 truncate font-medium">{m.title}</span>
                {#if session.recordingMeetingId === m.id}
                  <span class="flex items-center gap-1.5 text-xs font-semibold text-rec"><span class="rec-dot h-2 w-2 rounded-full bg-rec"></span>Live</span>
                {:else}
                  <span class="text-xs text-faint">{durationLabel(m.startedAt, m.endedAt)}</span>
                {/if}
              </button>
              <button
                class="mr-2 grid h-8 w-8 place-items-center rounded-full text-faint opacity-0 hover:bg-soft hover:text-rec focus-visible:opacity-100 group-hover:opacity-100"
                aria-label="Delete {m.title}"
                onclick={() => confirm(`Delete "${m.title}"? This can't be undone.`) && session.remove(m.id)}
              >
                <Icon name="delete" size={18} />
              </button>
            </li>
          {/each}
        </ul>
      {/each}
    {/if}
  </div>
</div>
