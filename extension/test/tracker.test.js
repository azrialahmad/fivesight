const test = require('node:test');
const assert = require('node:assert/strict');
const { createSpeakingTracker, createCallTracker, createChatTracker } = require('../src/tracker.js');

/** Feed `n` activity events into a tile, evenly across the 200 ms before `now`. */
function burst(t, id, now, n = 5) {
  for (let i = 0; i < n; i++) t.note(id, now - 200 + (i * 200) / n);
}

test('a tile with sustained activity becomes a speaker, then stops after quiet', () => {
  const t = createSpeakingTracker();
  const present = new Map([['a', 'Budi'], ['b', 'Sari']]);
  const edges = [];
  let now = 1000;
  for (let i = 0; i < 4; i++, now += 200) {
    burst(t, 'a', now);
    edges.push(...t.tick(now, present));
  }
  assert.deepEqual(edges, [{ name: 'Budi', speaking: true, at: 1200 }]);
  // Silence: needs ~1 s of quiet (5 ticks after the window empties) before it ends.
  const after = [];
  for (let i = 0; i < 12; i++, now += 200) after.push(...t.tick(now, present));
  assert.equal(after.length, 1);
  assert.equal(after[0].name, 'Budi');
  assert.equal(after[0].speaking, false);
});

test('a hover blip or one-off change does not name anyone', () => {
  const t = createSpeakingTracker();
  const present = new Map([['a', 'Budi']]);
  t.note('a', 990);
  t.note('a', 1000);
  let out = [];
  for (let now = 1000; now < 3000; now += 200) out.push(...t.tick(now, present));
  assert.deepEqual(out, []);
});

test('only the tile that is active is named', () => {
  const t = createSpeakingTracker();
  const present = new Map([['a', 'Budi'], ['b', 'Sari']]);
  const edges = [];
  for (let now = 1000; now < 2000; now += 200) {
    burst(t, 'b', now);
    edges.push(...t.tick(now, present));
  }
  assert.deepEqual(edges.map((e) => e.name), ['Sari']);
});

test('a tile that disappears while speaking ends its speech', () => {
  const t = createSpeakingTracker();
  const edges = [];
  let now = 1000;
  for (let i = 0; i < 3; i++, now += 200) {
    burst(t, 'a', now);
    edges.push(...t.tick(now, new Map([['a', 'Budi']])));
  }
  edges.push(...t.tick(now, new Map()));
  assert.deepEqual(edges.map((e) => [e.name, e.speaking]), [['Budi', true], ['Budi', false]]);
});

test('two tiles for one person (e.g. a presentation tile) report one speaker', () => {
  const t = createSpeakingTracker();
  const present = new Map([['a', 'Budi'], ['b', 'Budi']]);
  const edges = [];
  for (let now = 1000; now < 1800; now += 200) {
    burst(t, 'a', now);
    burst(t, 'b', now);
    edges.push(...t.tick(now, present));
  }
  assert.equal(edges.filter((e) => e.speaking).length, 1);
});

test('reset ends everyone who was speaking', () => {
  const t = createSpeakingTracker();
  const present = new Map([['a', 'Budi']]);
  for (let now = 1000; now < 1800; now += 200) {
    burst(t, 'a', now);
    t.tick(now, present);
  }
  assert.deepEqual(t.reset(5000), [{ name: 'Budi', speaking: false, at: 5000 }]);
  assert.deepEqual(t.reset(6000), []);
});

test('call tracker debounces joining and leaving', () => {
  const c = createCallTracker();
  assert.equal(c.tick(true), null);
  assert.equal(c.tick(true), 'joined');
  assert.equal(c.tick(true), null);
  // A single re-render with no leave button must not look like leaving.
  assert.equal(c.tick(false), null);
  assert.equal(c.tick(true), null);
  assert.equal(c.tick(false), null);
  assert.equal(c.tick(false), null);
  assert.equal(c.tick(false), 'left');
  assert.equal(c.inCall, false);
  assert.equal(c.tick(false), null);
});

test('closing the tab leaves immediately, once', () => {
  const c = createCallTracker();
  c.tick(true);
  c.tick(true);
  assert.equal(c.leaveNow(), 'left');
  assert.equal(c.leaveNow(), null);
});

test('tile notices are not taken for names', () => {
  const { looksLikeName } = require('../src/selectors.js');
  assert.equal(looksLikeName('M Alvin Wijaya'), true);
  assert.equal(looksLikeName('Muhammad Quwwamul Haq Djunaid'), true);
  assert.equal(looksLikeName('Others might still see your full video.'), false);
  assert.equal(looksLikeName('You are presenting to everyone now, please stop'), false);
});

test('chat messages are reported once, even when Meet redraws the panel', () => {
  const c = createChatTracker();
  const m = (id, at = null) => ({ id, sender: 'Budi', text: 'link: https://x/' + id, at, me: false });
  assert.deepEqual(
    c.fresh(10_000_000, [m('a')]).map((e) => [e.id, e.at]),
    [['a', 10_000_000]]
  );
  assert.deepEqual(c.fresh(10_001_000, [m('a'), m('b')]).map((e) => e.id), ['b']);
  assert.deepEqual(c.fresh(10_002_000, [m('a'), m('b')]), []);
  c.reset();
  assert.equal(c.fresh(10_003_000, [m('a')]).length, 1, 'a new call starts over');
});

test('a chat message found late keeps the time Meet printed; a live one is stamped now', () => {
  const c = createChatTracker();
  const now = 10_000_000;
  const [old, live, unknown] = c.fresh(now, [
    { id: 'old', sender: 'Sari', text: 'hi', at: now - 10 * 60_000 },
    { id: 'live', sender: 'Sari', text: 'yo', at: now - 30_000 },
    { id: 'unknown', sender: null, text: 'hm', at: null }
  ]);
  assert.equal(old.at, now - 10 * 60_000);
  assert.equal(live.at, now);
  assert.equal(unknown.at, now);
  assert.equal(c.fresh(now, [{ id: 'long', sender: 'S', text: 'x'.repeat(5000), at: null }])[0].text.length, 2000);
});
