<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  import Markdown from './Markdown.svelte';
  import { session } from './session.svelte';

  let text = $state('');
  let open = $state(true);
  let thread: HTMLDivElement | undefined = $state();

  /** The prompt being written, or null when the "Save a prompt" box is closed. */
  let draft = $state<string | null>(null);
  let draftInput: HTMLInputElement | undefined = $state();

  /** Starts from whatever is in the Ask box, to keep a question you just typed. */
  async function startDraft() {
    draft = text;
    await tick();
    draftInput?.focus();
  }

  function saveDraft() {
    if (draft?.trim()) session.addQuickPrompt(draft);
    draft = null;
  }

  const hasThread = $derived(session.chat.length > 0 || session.asking);

  /** Answer just added to the notes, to show "Added" on it for a moment. */
  let added = $state<number | null>(null);
  let addedTimer: ReturnType<typeof setTimeout>;
  function addToNotes(i: number, answer: string) {
    session.addToNotes(answer);
    added = i;
    clearTimeout(addedTimer);
    addedTimer = setTimeout(() => (added = null), 2000);
  }

  function submit(q: string = text) {
    if (!q.trim() || session.asking) return;
    session.ask(q);
    text = '';
    open = true;
  }

  $effect(() => {
    session.chat.length;
    session.answerLive.length;
    tick().then(() => thread?.scrollTo({ top: thread.scrollHeight }));
  });
</script>

<div class="border-t border-line bg-card px-5 pb-4 pt-3">
  {#if hasThread && open}
    <div bind:this={thread} class="mb-3 max-h-[30vh] space-y-3 overflow-y-auto pr-1">
      {#each session.chat as m, i}
        {#if m.role === 'user'}
          <div class="rise flex justify-end">
            <p class="max-w-[80%] rounded-2xl rounded-br-md bg-ink px-4 py-2 text-sm text-white">{m.content}</p>
          </div>
        {:else}
          <div class="rise max-w-[92%]">
            <div class="rounded-2xl rounded-bl-md bg-paper px-4 py-3"><Markdown text={m.content} /></div>
            <button
              class="mt-1 flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium text-muted hover:bg-soft hover:text-ink"
              title="Add this answer to the end of your notes, formatting and all. To add just part of it, select it, copy and paste it into your notes."
              onclick={() => addToNotes(i, m.content)}
            >
              <Icon name={added === i ? 'check' : 'plus'} size={13} />{added === i ? 'Added to notes' : 'Add to notes'}
            </button>
          </div>
        {/if}
      {/each}
      {#if session.lookups.length && (session.asking || session.chat.at(-1)?.role === 'assistant')}
        <div class="flex flex-wrap items-center gap-1.5 pl-1 text-[11px] text-muted" aria-live="polite">
          <span class="font-semibold tracking-wide text-faint">LOOKED UP</span>
          {#each session.lookups as l (l.id)}
            <span
              class="flex items-center gap-1 rounded-full px-2 py-0.5 font-medium {l.state === 'failed' ? 'bg-rec-soft text-rec' : 'bg-soft text-ink'}"
              title={l.error ?? ''}
            >
              {#if l.state === 'running'}<span class="h-2 w-2 animate-pulse rounded-full bg-lime-deep"></span>{:else if l.state === 'done'}<Icon name="check" size={12} />{/if}
              {l.label}{l.state === 'failed' ? ' (failed)' : ''}
            </span>
          {/each}
        </div>
      {/if}
      {#if session.answerSources.length && session.chat.at(-1)?.role === 'assistant' && !session.asking}
        <p class="flex flex-wrap items-center gap-1.5 pl-1 text-[11px] text-muted">
          <Icon name="book" size={13} /> From your files:
          {#each session.answerSources as f}<span class="rounded-full bg-soft px-2 py-0.5 font-medium text-ink">{f}</span>{/each}
        </p>
      {/if}
      {#if session.asking}
        <div class="max-w-[92%] rounded-2xl rounded-bl-md bg-paper px-4 py-3">
          {#if session.answerLive}
            <Markdown text={session.answerLive} />
          {:else}
            <div class="space-y-2" aria-label="Thinking">
              <div class="shimmer h-3 w-64 rounded-full"></div>
              <div class="shimmer h-3 w-44 rounded-full"></div>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  <div class="mb-2.5 flex max-h-20 flex-wrap items-center gap-2 overflow-y-auto">
    {#each session.settings.quickPrompts as c}
      <span class="group flex max-w-full items-center rounded-full border border-line bg-paper hover:border-ink">
        <button
          class="min-w-0 truncate rounded-full py-1.5 pl-3.5 pr-1 text-[13px] disabled:opacity-40"
          title={c}
          disabled={session.asking}
          onclick={() => submit(c)}>{c}</button
        >
        <button
          class="mr-1.5 grid h-5 w-5 shrink-0 place-items-center rounded-full text-muted opacity-0 hover:bg-soft hover:text-ink focus:opacity-100 group-hover:opacity-100"
          aria-label="Remove prompt: {c}"
          title="Remove this prompt"
          onclick={() => session.removeQuickPrompt(c)}
        >
          <Icon name="close" size={12} />
        </button>
      </span>
    {/each}
    {#if draft === null}
      <button class="flex items-center gap-1 rounded-full px-2.5 py-1.5 text-[13px] text-muted hover:bg-soft hover:text-ink" onclick={startDraft}>
        <Icon name="plus" size={14} /> Save a prompt
      </button>
    {:else}
      <form
        class="flex min-w-0 flex-1 basis-64 items-center gap-1 rounded-full border border-ink bg-paper py-0.5 pl-3.5 pr-0.5"
        onsubmit={(e) => {
          e.preventDefault();
          saveDraft();
        }}
      >
        <input
          bind:this={draftInput}
          class="min-w-0 flex-1 bg-transparent py-1 text-[13px] outline-none placeholder:text-faint"
          placeholder="A question you ask often…"
          aria-label="New prompt"
          bind:value={draft}
          onkeydown={(e) => e.key === 'Escape' && (draft = null)}
        />
        <button class="rounded-full bg-ink px-3 py-1 text-xs font-medium text-white disabled:opacity-30" disabled={!draft.trim()}>Save</button>
        <button type="button" class="rounded-full px-2 py-1 text-xs text-muted hover:bg-soft" onclick={() => (draft = null)}>Cancel</button>
      </form>
    {/if}
  </div>

  <form
    class="flex items-center gap-2 rounded-full border border-line bg-paper py-1.5 pl-5 pr-1.5 focus-within:border-ink"
    onsubmit={(e) => {
      e.preventDefault();
      submit();
    }}
  >
    <input
      class="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none placeholder:text-faint"
      placeholder={session.kb.length ? 'Ask about this meeting or your files…' : 'Ask about this meeting…'}
      aria-label="Ask about this meeting"
      bind:value={text}
    />
    {#if hasThread}
      <button type="button" class="rounded-full px-3 py-1.5 text-xs text-muted hover:bg-soft" onclick={() => (open = !open)}>
        {open ? 'Hide' : 'Show'} answers
      </button>
    {/if}
    <button
      type="submit"
      class="grid h-9 w-9 place-items-center rounded-full bg-ink text-white transition-opacity disabled:opacity-30"
      disabled={!text.trim() || session.asking}
      aria-label="Ask"
    >
      <Icon name="send" size={18} />
    </button>
  </form>
</div>
