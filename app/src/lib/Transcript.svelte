<script lang="ts">
  import { tick } from 'svelte';
  import { session } from './session.svelte';
  import { clock } from './format';

  let { compact = false, limit = 0 }: { compact?: boolean; limit?: number } = $props();

  let scroller: HTMLDivElement | undefined = $state();
  let pinned = true;

  const rows = $derived(limit > 0 ? session.sorted.slice(-limit) : session.sorted);

  function onScroll() {
    if (!scroller) return;
    pinned = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 64;
  }

  $effect(() => {
    // Re-run whenever text changes so partials keep the view pinned to the bottom.
    rows.map((r) => r.text.length);
    if (pinned) tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight, behavior: 'smooth' }));
  });
</script>

<div bind:this={scroller} onscroll={onScroll} class="h-full overflow-y-auto {compact ? 'px-4 py-3' : 'px-5 py-4'}">
  <ul class="flex flex-col {compact ? 'gap-4' : 'gap-5'}" aria-live="polite">
    {#each rows as r (r.key)}
      {@const me = r.source === 'mic'}
      <li class="rise flex gap-3">
        <span
          class="grid shrink-0 place-items-center rounded-full font-semibold {compact ? 'h-7 w-7 text-[11px]' : 'h-8 w-8 text-xs'} {me
            ? 'bg-lime text-ink'
            : 'bg-soft text-ink'}"
          aria-hidden="true">{me ? 'Y' : 'O'}</span
        >
        <div class="min-w-0 flex-1">
          <div class="flex items-baseline gap-2">
            <span class="text-xs font-semibold">{me ? 'You' : 'Others'}</span>
            <span class="text-[11px] tabular-nums text-faint">{clock(r.startMs)}</span>
          </div>
          <p class="mt-0.5 {compact ? 'text-[13px]' : 'text-sm'} leading-relaxed {r.final ? 'text-ink' : 'text-muted'}">
            {r.text}{#if !r.final}<span class="ml-0.5 inline-block h-3.5 w-0.5 translate-y-0.5 animate-pulse bg-ink"></span>{/if}
          </p>
        </div>
      </li>
    {/each}
  </ul>
</div>
