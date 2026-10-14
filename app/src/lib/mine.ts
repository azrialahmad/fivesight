// Tell the user's own lines apart from what the AI added, so the AI notes can show them
// differently: the prompt asks the model to copy each of the user's lines as it was typed,
// and this finds those lines again (allowing for a fixed typo or a dropped bullet).

/** Lowercase words of a line, without bullets, heading marks, numbering or punctuation. */
export function words(line: string): string[] {
  return line
    .toLowerCase()
    .replace(/^\s*(?:[-*+]|#{1,6}|\d+[.)])\s+/, '')
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
}

/** The user's note lines, ready for matching. */
export function noteLines(notes: string): string[][] {
  return notes.split('\n').map(words).filter((w) => w.length > 0);
}

/** Whether `text` is one of the user's lines: the same words, or nearly (one fixed typo). */
export function isMine(text: string, lines: string[][]): boolean {
  const mine = words(text);
  if (mine.length === 0) return false;
  const key = mine.join(' ');
  return lines.some((line) => {
    if (line.join(' ') === key) return true;
    if (line.length < 3 || mine.length < 3) return false;
    const a = new Set(line);
    const b = new Set(mine);
    let shared = 0;
    for (const w of a) if (b.has(w)) shared++;
    // 0.75: one word added or changed in a 5-word line still matches; an AI line that merely
    // reuses three of the user's words ("You: update the Confluence page") doesn't.
    return shared / (a.size + b.size - shared) >= 0.75;
  });
}

/** Add class "mine" to list items, headings and paragraphs whose own text is a user line. */
export function markMine(html: string, notes: string): string {
  const lines = noteLines(notes);
  if (lines.length === 0) return html;
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  for (const el of tpl.content.querySelectorAll('li, h1, h2, h3, h4, h5, h6, p')) {
    if (isMine(ownText(el), lines)) el.classList.add('mine');
  }
  return tpl.innerHTML;
}

/** Text of an element without nested lists (an item's sub-bullets) or citation chips. */
function ownText(el: Element): string {
  let out = '';
  for (const node of el.childNodes) {
    if (node.nodeType === Node.TEXT_NODE) out += node.textContent;
    else if (node instanceof Element && !/^(UL|OL)$/.test(node.tagName) && !node.classList.contains('cite')) {
      // A loose list item wraps its text in <p>; that <p> gets matched on its own.
      if (node.tagName !== 'P') out += ownText(node);
    }
  }
  return out;
}
