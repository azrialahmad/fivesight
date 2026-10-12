const test = require('node:test');
const assert = require('node:assert/strict');
const { createSpeakingTracker, createCallTracker } = require('../src/tracker.js');

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
