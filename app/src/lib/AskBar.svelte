<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  import Markdown from './Markdown.svelte';
  import { session } from './session.svelte';

  let text = $state('');
  let open = $state(true);
  let thread: HTMLDivElement | undefined = $state();

  const chips = ['What did we decide?', 'List the action items', 'Summarize in 3 bullets'];
  const hasThread = $derived(session.chat.length > 0 || session.asking);

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
      {#each session.chat as m}
        {#if m.role === 'user'}
          <div class="rise flex justify-end">
            <p class="max-w-[80%] rounded-2xl rounded-br-md bg-ink px-4 py-2 text-sm text-white">{m.content}</p>
          </div>
        {:else}
          <div class="rise max-w-[92%] rounded-2xl rounded-bl-md bg-paper px-4 py-3"><Markdown text={m.content} /></div>
        {/if}
      {/each}
      {#if session.lookups.length && (session.asking || session.chat.at(-1)?.role === 'assistant')}
        <div class="flex flex-wrap items-center gap-1.5 pl-1 text-[11px] text-muted" aria-live="polite">
          <span class="font-semibold tracking-wide text-faint">JIRA</span>
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

  {#if !hasThread}
    <div class="mb-2.5 flex flex-wrap gap-2">
      {#each chips as c}
        <button class="rounded-full border border-line bg-paper px-3.5 py-1.5 text-[13px] hover:border-ink" onclick={() => submit(c)}>{c}</button>
      {/each}
    </div>
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
