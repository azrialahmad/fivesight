const test = require('node:test');
const assert = require('node:assert/strict');
const { meetCode, parseChatTime, isOwnSender } = require('../src/selectors.js');

test('meetCode reads the code from both call address forms', () => {
  assert.equal(meetCode('/vzc-zjey-fxu'), 'vzc-zjey-fxu');
  assert.equal(meetCode('/meet/vzc-zjey-fxu'), 'vzc-zjey-fxu');
  assert.equal(meetCode('/vzc-zjey-fxu/'), 'vzc-zjey-fxu');
  assert.equal(meetCode('/'), null);
  assert.equal(meetCode('/landing'), null);
  assert.equal(meetCode('/meet/'), null);
});

test('parseChatTime reads the minute over a chat group, in 12 and 24 hour forms', () => {
  const now = new Date(2026, 10, 4, 14, 30, 15).getTime();
  const at = (h, m, day = 4) => new Date(2026, 10, day, h, m, 0, 0).getTime();
  assert.equal(parseChatTime('14:05', now), at(14, 5));
  assert.equal(parseChatTime('2:05 PM', now), at(14, 5));
  assert.equal(parseChatTime('12:10 am', now), at(0, 10));
  assert.equal(parseChatTime('9.45', now), at(9, 45));
  // Later than now: that was yesterday.
  assert.equal(parseChatTime('23:50', now), at(23, 50, 3));
  for (const bad of ['', null, 'Budi', '25:00', '13:00 PM', '10:42:11']) assert.equal(parseChatTime(bad, now), null);
});

test('isOwnSender knows "You" and your Meet name', () => {
  assert.ok(isOwnSender('You', null));
  assert.ok(isOwnSender('Anda', null));
  assert.ok(isOwnSender('Azrial', 'Azrial'));
  assert.ok(!isOwnSender('Budi', 'Azrial'));
  assert.ok(!isOwnSender(null, null));
});

test('pipDocument finds an open picture-in-picture window and ignores a closed one', () => {
  const { pipDocument } = require('../src/selectors.js');
  const doc = { id: 'pip' };
  assert.equal(pipDocument({ documentPictureInPicture: { window: { closed: false, document: doc } } }), doc);
  assert.equal(pipDocument({ documentPictureInPicture: { window: { closed: true, document: doc } } }), null);
  assert.equal(pipDocument({ documentPictureInPicture: { window: null } }), null);
  assert.equal(pipDocument({}), null); // a browser without Document Picture-in-Picture
  const throws = { get documentPictureInPicture() { throw new Error('blocked'); } };
  assert.equal(pipDocument(throws), null);
});

test('tileDocument reads tiles from picture-in-picture only while it shows some', () => {
  const { tileDocument } = require('../src/selectors.js');
  const docWith = (n) => ({ querySelectorAll: () => Array.from({ length: n }, () => ({})) });
  const tab = docWith(5);
  const pip = docWith(2);
  assert.equal(tileDocument(tab, null), tab);
  assert.equal(tileDocument(tab, pip), pip);
  // PiP still loading, or showing only a placeholder: stay on the tab.
  assert.equal(tileDocument(tab, docWith(0)), tab);
});
