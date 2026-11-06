// node --test test/quick.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { withPrompt } from '../src/lib/quick.ts';

test('a new prompt goes at the end, tidied', () => {
  assert.deepEqual(withPrompt(['a'], '  Who owns\n  what? '), ['a', 'Who owns what?']);
});

test('blank text and repeats change nothing', () => {
  const list = ['What did we decide?'];
  assert.equal(withPrompt(list, '   '), list);
  assert.equal(withPrompt(list, 'what did we DECIDE?'), list);
});
