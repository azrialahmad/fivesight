<script lang="ts">
  import { markMine } from './mine';
  import { renderMarkdown } from './markdown';

  /** `mine`: the user's own notes. Their lines are shown in ink, what the AI added in grey. */
  let { text, mine = '' }: { text: string; mine?: string } = $props();

  const html = $derived(markMine(renderMarkdown(text), mine));

  // A stray link must never navigate the app window away.
  function stopLinks(e: MouseEvent) {
    if ((e.target as HTMLElement).closest('a')) e.preventDefault();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="prose-notes" class:has-mine={mine.trim() !== ''} onclick={stopLinks}>{@html html}</div>

<style>
  :global(.prose-notes mark.cite) {
    background: var(--lime);
    color: var(--ink);
    border-radius: 4px;
    padding: 0 4px;
    font-size: 0.85em;
    font-variant-numeric: tabular-nums;
  }
</style>
