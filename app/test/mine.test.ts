// node --test test/mine.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isMine, noteLines, words } from '../src/lib/mine.ts';

const notes = noteLines('- PAY-142 timeout\n- retry = 30s?\n\n## Budi owns fix, Fri\n1. ask Maya about homepage changes\n- confluence page update');

test('words drop bullets, headings, numbering and punctuation', () => {
  assert.deepEqual(words('- PAY-142 timeout'), ['pay', '142', 'timeout']);
  assert.deepEqual(words('## Budi owns fix, Fri'), ['budi', 'owns', 'fix', 'fri']);
  assert.deepEqual(words('1. Deploy ke staging'), ['deploy', 'ke', 'staging']);
});

test("the user's own lines are recognised", () => {
  assert.ok(isMine('PAY-142 timeout', notes));
  assert.ok(isMine('retry = 30s?', notes));
  assert.ok(isMine('Budi owns fix, Fri', notes));
});

test('a fixed typo still counts as theirs', () => {
  assert.ok(isMine('Ask Maya about the homepage changes', notes));
});

test('AI additions are not theirs', () => {
  assert.ok(!isMine('The retry logic waits 30 seconds before failing', notes));
  assert.ok(!isMine('Action items', notes));
  assert.ok(!isMine('timeout', notes));
  // Reuses three of the user's words, but it's the AI's action item.
  assert.ok(!isMine('You: update the Confluence page', notes));
  assert.ok(!isMine('', notes));
});
