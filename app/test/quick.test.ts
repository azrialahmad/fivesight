// node --test test/quick.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chipLabel, withPrompt } from '../src/lib/quick.ts';

const sp = { label: 'SP', prompt: 'Estimate the story points' };

test('a new prompt goes at the end, tidied, and keeps its line breaks', () => {
  assert.deepEqual(withPrompt([sp], null, { label: ' Owners ', prompt: '  Who owns what?\nBy when? ' }), [
    sp,
    { label: 'Owners', prompt: 'Who owns what?\nBy when?' }
  ]);
});

test('editing replaces the prompt in place', () => {
  const other = { label: '', prompt: 'Other' };
  assert.deepEqual(withPrompt([sp, other], 0, { label: 'Points', prompt: 'New text' }), [{ label: 'Points', prompt: 'New text' }, other]);
  // Renaming keeps its own prompt: that isn't a repeat.
  assert.deepEqual(withPrompt([sp], 0, { ...sp, label: 'Points' }), [{ ...sp, label: 'Points' }]);
});

test('blank prompts and repeats change nothing', () => {
  const list = [sp, { label: '', prompt: 'Other' }];
  assert.equal(withPrompt(list, null, { label: 'x', prompt: '   ' }), list);
  assert.equal(withPrompt(list, null, { label: 'x', prompt: 'estimate the STORY points' }), list);
  assert.equal(withPrompt(list, 1, { label: 'x', prompt: sp.prompt }), list);
});

test('a button shows its name, or the prompt without one', () => {
  assert.equal(chipLabel(sp), 'SP');
  assert.equal(chipLabel({ label: '  ', prompt: 'Long question' }), 'Long question');
});
