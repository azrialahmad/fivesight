// HTML back to the Markdown fivesight keeps notes in: for the AI notes you edit in place, and for
// pasting formatted text (an Ask answer, a Confluence page, a chat message) into your notes.
// Covers what notes use: headings, paragraphs, nested lists, bold/italic/code, links, quotes,
// tables, and the [m:ss] citation chips. Anything else keeps its text.

/** The part of a DOM node this reads, so tests can pass plain objects. */
export interface HtmlNode {
  nodeType: number;
  nodeName: string;
  textContent: string | null;
  childNodes: ArrayLike<HtmlNode>;
  getAttribute?(name: string): string | null;
}

const TEXT = 3;
const ELEMENT = 1;
const BLOCKS = new Set([
  'P', 'DIV', 'SECTION', 'ARTICLE', 'HEADER', 'FOOTER', 'MAIN', 'ASIDE', 'NAV', 'FIGURE',
  'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'PRE', 'TABLE', 'HR', 'DL', 'DT', 'DD'
]);

const tag = (n: HtmlNode) => n.nodeName.toUpperCase();
const kids = (n: HtmlNode) => Array.from(n.childNodes);
const isBlock = (n: HtmlNode) => n.nodeType === ELEMENT && BLOCKS.has(tag(n));

/** Wrap inline text in a marker, keeping spaces outside it (`** x **` isn't bold). */
function wrap(inner: string, mark: string, end = mark): string {
  const t = inner.trim();
  if (!t) return inner;
  const lead = inner.slice(0, inner.length - inner.trimStart().length);
  const trail = inner.slice(inner.trimEnd().length);
  return `${lead}${mark}${t}${end}${trail}`;
}

function inline(n: HtmlNode): string {
  if (n.nodeType === TEXT) return (n.textContent ?? '').replace(/[\s ]+/g, ' ');
  if (n.nodeType !== ELEMENT) return '';
  const inner = () => kids(n).map(inline).join('');
  switch (tag(n)) {
    case 'BR':
      return '\n';
    case 'STRONG':
    case 'B':
      return wrap(inner(), '**');
    case 'EM':
    case 'I':
      return wrap(inner(), '*');
    case 'S':
    case 'DEL':
    case 'STRIKE':
      return wrap(inner(), '~~');
    case 'CODE':
      return wrap((n.textContent ?? '').replace(/\s+/g, ' '), '`');
    case 'MARK':
      // A citation chip shows "0:07"; the notes say "[0:07]".
      return /^\d{1,2}:\d{2}$/.test((n.textContent ?? '').trim()) ? `[${n.textContent!.trim()}]` : inner();
    case 'A': {
      const href = n.getAttribute?.('href') ?? '';
      const text = inner();
      return /^https?:\/\//i.test(href) && text.trim() && text.trim() !== href ? wrap(text, '[', `](${href})`) : text;
    }
    case 'SCRIPT':
    case 'STYLE':
    case 'TEMPLATE':
      return '';
    default:
      return inner();
  }
}

/** Inline text cleaned up line by line. */
function tidy(s: string): string {
  return s
    .split('\n')
    .map((l) => l.trim())
    .join('\n')
    .replace(/\n{2,}/g, '\n')
    .trim();
}

/** Markdown blocks for a run of sibling nodes: inline runs become paragraphs. */
function blocks(nodes: HtmlNode[]): string[] {
  const out: string[] = [];
  let run = '';
  const flush = () => {
    const t = tidy(run);
    if (t) out.push(t);
    run = '';
  };
  for (const n of nodes) {
    if (isBlock(n)) {
      flush();
      const b = block(n);
      if (b) out.push(b);
    } else run += inline(n);
  }
  flush();
  return out;
}

function indent(text: string, by: string): string {
  return text
    .split('\n')
    .map((l) => (l ? by + l : l))
    .join('\n');
}

function list(n: HtmlNode, ordered: boolean): string {
  const items: string[] = [];
  let i = 1;
  for (const c of kids(n)) {
    if (c.nodeType !== ELEMENT) continue;
    const t = tag(c);
    if (t === 'UL' || t === 'OL') {
      // An indented list right inside a list (how editors nest): it belongs to the item before.
      const nested = list(c, t === 'OL');
      if (!nested) continue;
      if (items.length) items[items.length - 1] += '\n' + indent(nested, ordered ? '   ' : '  ');
      else items.push(nested);
      continue;
    }
    if (t !== 'LI') continue;
    const parts = blocks(kids(c));
    if (!parts.length) continue;
    const marker = ordered ? `${i++}. ` : '- ';
    const body = parts.join('\n');
    items.push(marker + indent(body, ' '.repeat(marker.length)).slice(marker.length));
  }
  return items.join('\n');
}

function table(n: HtmlNode): string {
  const rows: string[][] = [];
  const walk = (x: HtmlNode) => {
    for (const c of kids(x)) {
      if (c.nodeType !== ELEMENT) continue;
      if (tag(c) === 'TR') {
        rows.push(
          kids(c)
            .filter((d) => d.nodeType === ELEMENT && (tag(d) === 'TD' || tag(d) === 'TH'))
            .map((d) => blocks(kids(d)).join(' ').replace(/\n/g, ' ').replace(/\|/g, '\\|'))
        );
      } else walk(c);
    }
  };
  walk(n);
  if (!rows.length) return '';
  const width = Math.max(...rows.map((r) => r.length));
  const line = (r: string[]) => `| ${Array.from({ length: width }, (_, i) => r[i] ?? '').join(' | ')} |`;
  return [line(rows[0]), `|${' --- |'.repeat(width)}`, ...rows.slice(1).map(line)].join('\n');
}

function block(n: HtmlNode): string {
  const t = tag(n);
  if (/^H[1-6]$/.test(t)) {
    const text = tidy(kids(n).map(inline).join('')).replace(/\n/g, ' ');
    return text ? `${'#'.repeat(Number(t[1]))} ${text}` : '';
  }
  switch (t) {
    case 'UL':
      return list(n, false);
    case 'OL':
      return list(n, true);
    case 'LI':
      return list({ ...n, nodeName: 'UL', childNodes: [n] }, false);
    case 'BLOCKQUOTE':
      return blocks(kids(n))
        .join('\n\n')
        .split('\n')
        .map((l) => (l ? `> ${l}` : '>'))
        .join('\n');
    case 'PRE': {
      const code = (n.textContent ?? '').replace(/\n+$/, '');
      return code.trim() ? '```\n' + code + '\n```' : '';
    }
    case 'HR':
      return '---';
    case 'TABLE':
      return table(n);
    default:
      return blocks(kids(n)).join('\n\n');
  }
}

/** Markdown for an element's contents (an editor, or a fragment of pasted HTML). */
export function htmlToMarkdown(root: HtmlNode): string {
  return blocks(kids(root))
    .join('\n\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}
