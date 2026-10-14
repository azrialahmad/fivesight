<script lang="ts">
  import Icon from './Icon.svelte';
  import { session, inTauri } from './session.svelte';
  import { dayLabel } from './format';

  let over = $state(false);

  function size(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }

  const name = (p: string) => p.split(/[\\/]/).pop() || p;
  const added = (t: number) => {
    const l = dayLabel(t);
    return /^(today|yesterday)$/i.test(l) ? l.toLowerCase() : l;
  };
  const totalPassages = $derived(session.kb.reduce((n, d) => n + d.chunks, 0));
</script>

<div
  class="h-full overflow-y-auto"
  role="region"
  aria-label="Your files"
  ondragenter={() => (over = true)}
  ondragover={(e) => {
    e.preventDefault();
    over = true;
  }}
  ondragleave={(e) => {
    if (e.currentTarget === e.target) over = false;
  }}
  ondrop={(e) => {
    e.preventDefault();
    over = false;
  }}
>
  <div class="mx-auto max-w-2xl px-8 pb-12 pt-8">
    <div class="flex items-end gap-4">
      <div class="min-w-0">
        <h1 class="text-2xl font-semibold tracking-tight">Your files</h1>
        <p class="mt-1 text-sm text-muted">
          Runbooks, specs, notes. When you ask a question, fivesight looks here too and says which file it used.
        </p>
      </div>
      <button
        class="ml-auto flex shrink-0 items-center gap-2 rounded-full bg-ink px-5 py-2.5 text-sm font-semibold text-white disabled:opacity-60"
        disabled={session.kbBusy}
        onclick={() => session.pickFiles()}
      >
        <span class="grid h-5 w-5 place-items-center rounded-full bg-lime text-ink"><Icon name="plus" size={14} /></span>
        {session.kbBusy ? 'Reading…' : 'Add files'}
      </button>
    </div>

    <div
      class="mt-6 rounded-2xl border-2 border-dashed px-6 py-5 text-center transition-colors {over ? 'border-ink bg-lime/25' : 'border-line bg-paper/60'}"
    >
      <span class="mx-auto grid h-10 w-10 place-items-center rounded-xl bg-card text-ink shadow-sm"><Icon name="upload" size={20} /></span>
      <p class="mt-2 text-sm font-medium">{over ? 'Drop to add' : 'Drop files here'}</p>
      <p class="mt-0.5 text-xs text-muted">Markdown, text and PDF{inTauri ? '' : ' (preview: dropping is only wired in the app)'}. Files stay on this PC; only the passages that match a question are sent to your AI model.</p>
    </div>

    {#if session.kbErrors.length}
      <div class="mt-4 space-y-1 rounded-xl bg-rec-soft px-4 py-3 text-sm text-rec" role="alert">
        {#each session.kbErrors as e}<p>{e.error}</p>{/each}
      </div>
    {/if}

    {#if session.kb.length}
      <div class="mt-8 flex items-baseline">
        <h2 class="text-[11px] font-semibold tracking-wider text-faint">{session.kb.length} {session.kb.length === 1 ? 'FILE' : 'FILES'}</h2>
        <span class="ml-auto text-xs text-faint">{totalPassages} passages indexed</span>
      </div>
      <ul class="mt-2 divide-y divide-line rounded-2xl border border-line">
        {#each session.kb as d (d.id)}
          <li class="rise group flex items-center gap-3.5 px-4 py-3">
            <span
              class="grid h-9 w-9 shrink-0 place-items-center rounded-lg text-[10px] font-bold uppercase tracking-wide {d.kind === 'pdf' ? 'bg-rec-soft text-rec' : d.kind === 'md' ? 'bg-lime text-ink' : 'bg-soft text-ink'}"
              aria-hidden="true">{d.kind}</span
            >
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm font-medium">{d.title}</p>
              <p class="truncate text-xs text-muted" title={d.path}>{name(d.path)} · {size(d.bytes)} · added {added(d.addedAt)}</p>
            </div>
            <div class="flex shrink-0 gap-1 opacity-60 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
              <button class="grid h-8 w-8 place-items-center rounded-full text-muted hover:bg-soft hover:text-ink" title="Read the file again" aria-label="Refresh {d.title}" onclick={() => session.refreshDoc(d.id)}>
                <Icon name="refresh" size={17} />
              </button>
              <button class="grid h-8 w-8 place-items-center rounded-full text-muted hover:bg-rec-soft hover:text-rec" title="Remove from fivesight (the file itself stays)" aria-label="Remove {d.title}" onclick={() => session.removeDoc(d.id)}>
                <Icon name="delete" size={17} />
              </button>
            </div>
          </li>
        {/each}
      </ul>
      <p class="mt-3 text-xs text-faint">Changed a file? Press refresh to read it again. Removing only takes it out of fivesight.</p>
    {:else}
      <p class="mt-8 text-center text-sm text-muted">No files yet. Add the docs you keep looking up during meetings.</p>
    {/if}
  </div>
</div>
