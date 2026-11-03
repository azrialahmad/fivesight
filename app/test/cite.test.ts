// node --test test/cite.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { citeChips } from '../src/lib/cite.ts';

const chip = (t: string) => `<mark class="cite">${t}</mark>`;

test('a plain citation becomes a chip', () => {
  assert.equal(citeChips('Retry waits 30 s [0:07]'), `Retry waits 30 s ${chip('0:07')}`);
  assert.equal(citeChips('[12:50][13:05]'), chip('12:50') + chip('13:05'));
});

test('the forms models write instead still become chips', () => {
  assert.equal(citeChips('x [12:50, 13:05]'), `x ${chip('12:50')} ${chip('13:05')}`);
  assert.equal(citeChips('x [12:50; 13:05 and 14:00]'), `x ${chip('12:50')} ${chip('13:05')} ${chip('14:00')}`);
  assert.equal(citeChips('x [12:50 - 13:10]'), `x ${chip('12:50')}–${chip('13:10')}`);
  assert.equal(citeChips('x [12:50–13:10]'), `x ${chip('12:50')}–${chip('13:10')}`);
  assert.equal(citeChips(String.raw`x \[12:50\]`), `x ${chip('12:50')}`);
  assert.equal(citeChips('x [ 12:50 ]'), `x ${chip('12:50')}`);
  assert.equal(citeChips('long meeting [105:20]'), `long meeting ${chip('105:20')}`);
  assert.equal(citeChips('[1:05:20]'), chip('1:05:20'));
});

test('other brackets are left alone', () => {
  for (const t of ['[Payments runbook]', '[PAY-142]', '[12:5]', '[x 12:50]', 'at 12:50 today', '[12:50 today]']) {
    assert.equal(citeChips(t), t);
  }
});
