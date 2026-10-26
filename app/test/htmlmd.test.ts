// node --test test/htmlmd.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { htmlToMarkdown, type HtmlNode } from '../src/lib/htmlmd.ts';

// A tiny HTML reader for the tests (no DOM in Node): tags, attributes, text, a few entities.
function parse(html: string): HtmlNode {
  const root: HtmlNode & { childNodes: HtmlNode[] } = { nodeType: 1, nodeName: 'BODY', textContent: '', childNodes: [] };
  const stack = [root];
  const re = /<(\/?)([a-zA-Z0-9]+)([^>]*)>|([^<]+)/g;
  const voids = new Set(['BR', 'HR']);
  for (const [, close, name, attrs, text] of html.matchAll(re)) {
    const top = stack[stack.length - 1];
    if (text !== undefined) {
      const t = text.replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
      top.childNodes.push({ nodeType: 3, nodeName: '#text', textContent: t, childNodes: [] });
    } else if (close) {
      if (stack.length > 1) stack.pop();
    } else {
      const a = Object.fromEntries([...attrs.matchAll(/(\w+)="([^"]*)"/g)].map((m) => [m[1], m[2]]));
      const el = {
        nodeType: 1,
        nodeName: name.toUpperCase(),
        childNodes: [] as HtmlNode[],
        getAttribute: (k: string) => a[k] ?? null,
        get textContent(): string {
          return this.childNodes.map((c) => c.textContent).join('');
        }
      };
      top.childNodes.push(el);
      if (!voids.has(el.nodeName)) stack.push(el);
    }
  }
  return root;
}

const md = (html: string) => htmlToMarkdown(parse(html));

test('headings, paragraphs and inline formatting', () => {
  assert.equal(
    md('<h2>PAY-142 timeout</h2><p>The retry waits <strong>30 seconds</strong> and <em>then</em> fails.</p>'),
    '## PAY-142 timeout\n\nThe retry waits **30 seconds** and *then* fails.'
  );
  assert.equal(md('<p><b>Owner: </b>Budi</p>'), '**Owner:** Budi');
  assert.equal(md('<p>run <code>npm ci</code></p>'), 'run `npm ci`');
});

test('nested lists keep their shape', () => {
  assert.equal(
    md('<ul>\n<li>PAY-142 timeout<ul>\n<li>reproduced on staging</li>\n<li>retry waits 30s</li></ul></li>\n<li>Budi owns fix</li></ul>'),
    '- PAY-142 timeout\n  - reproduced on staging\n  - retry waits 30s\n- Budi owns fix'
  );
  assert.equal(md('<ol><li>first</li><li>second<ol><li>inner</li></ol></li></ol>'), '1. first\n2. second\n   1. inner');
});

test('lists nested the way editors do it, and loose items', () => {
  assert.equal(md('<ul><li>a</li><ul><li>b</li></ul><li>c</li></ul>'), '- a\n  - b\n- c');
  assert.equal(md('<ul><li><p>a</p></li><li><p>b</p></li></ul>'), '- a\n- b');
  assert.equal(md('<ul><li>a</li><li><br></li></ul>'), '- a');
});

test('citation chips become [m:ss] again', () => {
  assert.equal(md('<ul><li>Retry waits 30s <mark class="cite" contenteditable="false">0:07</mark></li></ul>'), '- Retry waits 30s [0:07]');
  assert.equal(md('<p><mark>highlighted</mark></p>'), 'highlighted');
});

test('links only when they go somewhere real', () => {
  assert.equal(md('<p>See <a href="https://x.atlassian.net/wiki/1">the PRD</a></p>'), 'See [the PRD](https://x.atlassian.net/wiki/1)');
  assert.equal(md('<p><a href="javascript:alert(1)">click</a></p>'), 'click');
});

test('line breaks, divs and nbsp from an editor', () => {
  assert.equal(md('<div>one</div><div>two&nbsp;words</div>'), 'one\n\ntwo words');
  assert.equal(md('<p>line one<br>line two</p>'), 'line one\nline two');
  assert.equal(md('<p></p><p>  </p>'), '');
});

test('quotes and tables', () => {
  assert.equal(md('<blockquote><p>We ship Friday.</p></blockquote>'), '> We ship Friday.');
  assert.equal(
    md('<table><thead><tr><th>AC</th><th>Status</th></tr></thead><tbody><tr><td>AC1</td><td>done</td></tr></tbody></table>'),
    '| AC | Status |\n| --- | --- |\n| AC1 | done |'
  );
});

test('a bare list item from a partial selection', () => {
  assert.equal(md('<li>just this one</li>'), '- just this one');
});
