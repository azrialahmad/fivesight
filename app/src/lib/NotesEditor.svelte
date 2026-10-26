<script lang="ts">
  import { htmlToMarkdown } from './htmlmd';
  import { pastedMarkdown, renderMarkdown } from './markdown';

  // Notes you edit as they look (headings, bullets, bold), saved as Markdown. New `text` from
  // outside (the AI's next update) replaces what's shown; your own typing doesn't re-render.
  let {
    text,
    onedit,
    placeholder = '',
    label
  }: { text: string; onedit: (markdown: string) => void; placeholder?: string; label: string } = $props();

  let el = $state<HTMLDivElement>();
  let empty = $state(true);
  /** The Markdown the editor shows right now, as last rendered or typed. */
  let shown: string | null = null;

  $effect(() => {
    const t = text;
    if (!el || t === shown) return;
    const caret = document.activeElement === el ? caretOffset(el) : null;
    el.innerHTML = renderMarkdown(t, true);
    shown = t;
    empty = !t.trim();
    if (caret !== null) placeCaret(el, caret);
  });

  function changed() {
    if (!el) return;
    const md = htmlToMarkdown(el);
    empty = !md && !el.textContent?.trim();
    if (md === shown) return;
    shown = md;
    onedit(md);
  }

  function paste(e: ClipboardEvent) {
    const md = pastedMarkdown(e.clipboardData);
    if (md === null) return; // plain text: the browser inserts it as typed
    e.preventDefault();
    // execCommand keeps the paste on the undo stack, unlike setting the HTML directly.
    document.execCommand('insertHTML', false, renderMarkdown(md, true));
  }

  /** Characters of text before the caret, to put it back after the notes are re-rendered. */
  function caretOffset(root: HTMLElement): number | null {
    const sel = getSelection();
    if (!sel?.rangeCount) return null;
    const at = sel.getRangeAt(0);
    const r = document.createRange();
    r.selectNodeContents(root);
    r.setEnd(at.endContainer, at.endOffset);
    return r.toString().length;
  }

  function placeCaret(root: HTMLElement, offset: number) {
    const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const r = document.createRange();
    r.selectNodeContents(root);
    r.collapse(false);
    for (let n = walk.nextNode(); n; n = walk.nextNode()) {
      const len = n.textContent?.length ?? 0;
      if (offset <= len) {
        r.setStart(n, offset);
        r.collapse(true);
        break;
      }
      offset -= len;
    }
    getSelection()?.removeAllRanges();
    getSelection()?.addRange(r);
  }

  // A stray link must never navigate the app window away.
  function stopLinks(e: MouseEvent) {
    if ((e.target as HTMLElement).closest('a')) e.preventDefault();
  }
</script>

<div class="relative">
  {#if empty}
    <p class="pointer-events-none absolute inset-x-0 top-0 text-[15px] leading-relaxed text-faint" aria-hidden="true">{placeholder}</p>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    bind:this={el}
    class="prose-notes min-h-[8rem] outline-none"
    contenteditable="true"
    role="textbox"
    tabindex="0"
    aria-multiline="true"
    aria-label={label}
    oninput={changed}
    onpaste={paste}
    onclick={stopLinks}
    onfocus={() => document.execCommand('defaultParagraphSeparator', false, 'p')}
  ></div>
</div>
