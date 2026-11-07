// Models write Markdown tables in ways a strict parser leaves as raw "|" text. This repairs the
// common ones before rendering. Anything else, and everything in a real code block, is left alone.

const ROW = /^\s*\|.*\|\s*$/;
const SEPARATOR = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
const FENCE = /^\s*```/;

const isSeparator = (line: string) => line.includes('|') && SEPARATOR.test(line);

/** How many cells a row has. An escaped \| stays inside its cell. */
function cellCount(row: string): number {
  const cells = row.trim().replace(/^\|/, '').replace(/(?<!\\)\|$/, '');
  return cells.split(/(?<!\\)\|/).length;
}

/** A whole table wrapped in a code fence, as if showing how to write one: unwrap it. */
function unfence(text: string): string {
  return text.replace(/^[ \t]*```(?:markdown|md)?[ \t]*\n([\s\S]*?)^[ \t]*```[ \t]*$/gm, (block, body: string) => {
    const lines = body.split('\n').filter((l) => l.trim());
    return lines.length >= 2 && lines.every((l) => ROW.test(l)) && lines.some(isSeparator) ? body.trimEnd() : block;
  });
}

export function tidyTables(markdown: string): string {
  const lines = unfence(markdown.replace(/\r\n/g, '\n')).split('\n');
  const nextFilled = (from: number) => {
    let i = from;
    while (i < lines.length && !lines[i].trim()) i++;
    return i;
  };
  const out: string[] = [];
  let inCode = false;
  lines.forEach((line, i) => {
    if (FENCE.test(line)) inCode = !inCode;
    if (inCode) return void out.push(line);

    const before = out.at(-1) ?? '';
    if (!line.trim() && ROW.test(before)) {
      // A blank line between rows ends the table early. Unless what follows starts a new table.
      const next = nextFilled(i);
      const afterNext = lines[nextFilled(next + 1)] ?? '';
      if (ROW.test(lines[next] ?? '') && !isSeparator(afterNext)) return;
    }
    // A separator with the wrong number of columns stops the whole thing being a table.
    if (isSeparator(line) && ROW.test(before) && cellCount(line) !== cellCount(before)) {
      return void out.push(`|${'---|'.repeat(cellCount(before))}`);
    }
    out.push(line);
  });
  return out.join('\n');
}
