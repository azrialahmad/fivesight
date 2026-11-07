// node --test test/tables.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { marked } from 'marked';
import { tidyTables } from '../src/lib/tables.ts';

const isTable = (md: string) => (marked.parse(tidyTables(md), { async: false, breaks: true }) as string).includes('<table>');
const good = '| A | B |\n|---|---|\n| 1 | 2 |';

test('a good table is left exactly as it is', () => {
  assert.equal(tidyTables(`Before\n\n${good}\n\nAfter`), `Before\n\n${good}\n\nAfter`);
});

test('a table wrapped in a code fence is unwrapped', () => {
  for (const fence of ['```markdown', '```md', '```']) {
    assert.equal(tidyTables(`${fence}\n${good}\n\`\`\``), good);
  }
});

test('real code keeps its fence, pipes and all', () => {
  const code = '```bash\ncat x | grep y |\n|---|\n```';
  assert.equal(tidyTables(code), code);
  const pipes = '```\na | b\n```';
  assert.equal(tidyTables(pipes), pipes);
});

test('blank lines between rows are removed', () => {
  assert.ok(isTable('| A | B |\n\n|---|---|\n\n| 1 | 2 |\n\n| 3 | 4 |'));
  const html = marked.parse(tidyTables('| A | B |\n\n|---|---|\n\n| 1 | 2 |\n\n| 3 | 4 |'), { async: false }) as string;
  assert.ok(html.includes('<td>3</td>') && !html.includes('<p>'), html);
});

test('two tables in a row stay two tables', () => {
  const two = `${good}\n\n| C | D |\n|---|---|\n| 5 | 6 |`;
  assert.equal(tidyTables(two), two);
});

test('a separator with the wrong number of columns is rebuilt', () => {
  assert.ok(isTable('| A | B | C |\n|---|---|\n| 1 | 2 | 3 |'));
  assert.equal(tidyTables('| A | B | C |\n|---|\n| 1 | 2 | 3 |'), '| A | B | C |\n|---|---|---|\n| 1 | 2 | 3 |');
  // An escaped pipe is part of a cell, not a new column.
  assert.equal(tidyTables('| A \\| x | B |\n|---|---|'), '| A \\| x | B |\n|---|---|');
});

test('Windows line endings and text around a table are fine', () => {
  assert.ok(isTable('| A | B |\r\n|---|---|\r\n| 1 | 2 |\r\n'));
  assert.ok(isTable(`Here it is:\n${good}\nDone`));
});

test('plain text with pipes is not touched', () => {
  const text = 'Use a | b for this.\n\nThen | c.';
  assert.equal(tidyTables(text), text);
  assert.equal(tidyTables('---'), '---');
});
