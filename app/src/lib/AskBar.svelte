<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  import Markdown from './Markdown.svelte';
  import { session } from './session.svelte';
  import { chipLabel } from './quick';

  let open = $state(true);
  /** The question whose Rewind was pressed, waiting for a yes. */
  let rewinding = $state<number | null>(null);
  let thread: HTMLDivElement | undefined = $state();

  /** The prompt being added (`index` null) or edited, or null when the editor is closed. */
  let draft = $state<{ index: number | null; label: string; prompt: string } | null>(null);
  let draftInput: HTMLInputElement | undefined = $state();

  async function openDraft(index: number | null, label: string, prompt: string) {
    draft = { index, label, prompt };
    await tick();
    draftInput?.focus();
  }

  function saveDraft() {
    if (draft?.prompt.trim()) session.saveQuickPrompt(draft.index, { label: draft.label, prompt: draft.prompt });
    draft = null;
  }

  function removeDraft() {
    if (draft?.index != null) session.removeQuickPrompt(draft.index);
    draft = null;
  }

  function draftKey(e: KeyboardEvent) {
    if (e.key === 'Escape') draft = null;
    else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) saveDraft();
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

  function submit(q: string = session.askText, fromBox = true) {
    if (!q.trim() || session.asking) return;
    session.ask(q);
    if (fromBox) session.askText = '';
    open = true;
    rewinding = null;
  }

  $effect(() => {
    session.chat.length;
    session.answerLive.length;
    tick().then(() => thread?.scrollTo({ top: thread.scrollHeight }));
  });
</script>

<div class="border-t border-line bg-card px-5 pb-4 pt-3">
  {#if hasThread && open && !draft}
    <div bind:this={thread} class="mb-3 max-h-[30vh] space-y-3 overflow-y-auto pr-1">
      {#each session.chat as m, i}
        {#if m.role === 'user'}
          <div class="rise group flex flex-col items-end">
            <p class="max-w-[80%] whitespace-pre-wrap break-words rounded-2xl rounded-br-md bg-ink px-4 py-2 text-sm text-white">{m.content}</p>
            {#if rewinding === i}
              <p class="mt-1 flex flex-wrap items-center justify-end gap-1 text-[11px] text-muted">
                {session.chat.length - i > 1 ? `Remove this and the ${session.chat.length - i - 1} after it?` : 'Remove this question?'}
                <button class="rounded-full bg-ink px-2 py-0.5 font-medium text-white" onclick={() => { session.rewindTo(i); rewinding = null; }}>Rewind</button>
                <button class="rounded-full px-2 py-0.5 hover:bg-soft" onclick={() => (rewinding = null)}>Cancel</button>
              </p>
            {:else if !session.asking}
              <button
                class="mt-1 rounded-full px-2 py-0.5 text-[11px] font-medium text-muted opacity-0 hover:bg-soft hover:text-ink focus:opacity-100 group-hover:opacity-100"
                title="Go back to before this question, to ask it again or differently"
                onclick={() => (rewinding = i)}
              >
                Rewind to here
              </button>
            {/if}
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
    {#each session.settings.quickPrompts as c, i}
      {@const name = chipLabel(c)}
      <span class="group relative max-w-64 rounded-full border border-line bg-paper hover:border-ink">
        <button
          class="block w-full truncate rounded-full px-3.5 py-1.5 text-[13px] disabled:opacity-40"
          title={c.prompt}
          disabled={session.asking}
          onclick={() => submit(c.prompt, false)}>{name}</button
        >
        <!-- Over the button's padding, so it takes no room until it's wanted. -->
        <button
          class="absolute right-1 top-1/2 grid h-5 w-5 -translate-y-1/2 place-items-center rounded-full bg-paper text-muted opacity-0 hover:bg-soft hover:text-ink focus:opacity-100 group-hover:opacity-100"
          aria-label="Edit prompt: {name}"
          title="Edit or remove this prompt"
          onclick={() => openDraft(i, c.label, c.prompt)}
        >
          <Icon name="edit" size={12} />
        </button>
      </span>
    {/each}
    {#if draft === null}
      <!-- Starts from whatever is in the Ask box, to keep a question you just typed. -->
      <button class="flex items-center gap-1 rounded-full px-2.5 py-1.5 text-[13px] text-muted hover:bg-soft hover:text-ink" onclick={() => openDraft(null, '', session.askText)}>
        <Icon name="plus" size={14} /> Save a prompt
      </button>
    {/if}
  </div>

  {#if draft}
    <form
      class="mb-2.5 rounded-2xl border border-ink bg-paper p-3"
      onsubmit={(e) => {
        e.preventDefault();
        saveDraft();
      }}
    >
      <input
        bind:this={draftInput}
        class="w-full bg-transparent text-[13px] font-medium outline-none placeholder:font-normal placeholder:text-faint"
        placeholder="Button name, such as SP estimate"
        aria-label="Button name"
        bind:value={draft.label}
        onkeydown={draftKey}
      />
      <textarea
        class="mt-2 w-full resize-y border-t border-line bg-transparent pt-2 text-[13px] leading-relaxed outline-none placeholder:text-faint"
        rows="3"
        placeholder="The full prompt the AI gets when you click it"
        aria-label="Prompt"
        bind:value={draft.prompt}
        onkeydown={draftKey}
      ></textarea>
      <div class="mt-1 flex items-center justify-end gap-1">
        {#if draft.index !== null}
          <button type="button" class="mr-auto rounded-full px-2.5 py-1 text-xs font-medium text-rec hover:bg-rec-soft" onclick={removeDraft}>Remove</button>
        {/if}
        <button type="button" class="rounded-full px-2.5 py-1 text-xs text-muted hover:bg-soft" onclick={() => (draft = null)}>Cancel</button>
        <button class="rounded-full bg-ink px-3 py-1 text-xs font-medium text-white disabled:opacity-30" disabled={!draft.prompt.trim()}>Save</button>
      </div>
    </form>
  {/if}

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
      bind:value={session.askText}
    />
    {#if session.settings.profiles.length > 1}
      <select
        class="max-w-36 truncate rounded-full bg-transparent py-1.5 pl-2 text-xs text-muted outline-none hover:bg-soft focus:bg-soft"
        title="AI profile for Ask and the AI notes. Set them up in Settings."
        aria-label="AI profile"
        value={session.settings.activeProfile}
        onchange={(e) => session.setActiveProfile(e.currentTarget.value)}
      >
        {#each session.settings.profiles as p}<option value={p.id}>{p.name || p.baseUrl || 'Untitled'}</option>{/each}
      </select>
    {/if}
    {#if hasThread}
      <button type="button" class="rounded-full px-3 py-1.5 text-xs text-muted hover:bg-soft" onclick={() => (open = !open)}>
        {open ? 'Hide' : 'Show'} answers
      </button>
    {/if}
    {#if session.asking}
      <button
        type="button"
        class="grid h-9 w-9 place-items-center rounded-full bg-ink text-white"
        aria-label="Stop the answer"
        title="Stop the answer"
        onclick={() => session.stopAsk()}
      >
        <Icon name="stop" size={18} />
      </button>
    {:else}
      <button
        type="submit"
        class="grid h-9 w-9 place-items-center rounded-full bg-ink text-white transition-opacity disabled:opacity-30"
        disabled={!session.askText.trim()}
        aria-label="Ask"
      >
        <Icon name="send" size={18} />
      </button>
    {/if}
  </form>
</div>
