// Transcript citations in notes and answers: "[12:50]" becomes a highlighter chip. Models don't
// always write the plain form, so these count too: one bracket holding several times or a range
// ("[12:50, 13:05]", "[12:50–13:10]"), Markdown-escaped brackets ("\[12:50\]"), spaces inside
// them, meetings past 99 minutes ("[105:20]") and hour clocks ("[1:05:20]").

const TIME = String.raw`\d{1,3}:\d{2}(?::\d{2})?`;
const SEP = String.raw`\s*(?:[,;&/–—-]|\band\b|\bdan\b)\s*`;
const GROUP = new RegExp(String.raw`\\?\[\s*(${TIME}(?:${SEP}${TIME})*)\s*\\?\]`, 'g');
const TIME_RE = new RegExp(TIME, 'g');

/** Every citation as `<mark class="cite">m:ss</mark>`, one chip per time; a range keeps its dash. */
export function citeChips(text: string): string {
  return text.replace(GROUP, (_, inner: string) => {
    let out = '';
    let last = 0;
    for (const m of inner.matchAll(TIME_RE)) {
      if (out) out += /[-–—]/.test(inner.slice(last, m.index)) ? '–' : ' ';
      out += `<mark class="cite">${m[0]}</mark>`;
      last = m.index! + m[0].length;
    }
    return out;
  });
}
