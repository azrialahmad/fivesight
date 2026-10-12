<script lang="ts">
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';

  let { text }: { text: string } = $props();

  // Model output is untrusted: always sanitize before it reaches {@html}.
  // [m:ss] citations become highlighter chips.
  const html = $derived(
    DOMPurify.sanitize(
      marked.parse(text.replace(/\[(\d{1,2}:\d{2})\]/g, '<mark class="cite">$1</mark>'), { async: false, breaks: true }) as string
    )
  );

  // A stray link must never navigate the app window away.
  function stopLinks(e: MouseEvent) {
    if ((e.target as HTMLElement).closest('a')) e.preventDefault();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="prose-notes" onclick={stopLinks}>{@html html}</div>

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
