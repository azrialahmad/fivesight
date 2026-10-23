const test = require('node:test');
const assert = require('node:assert/strict');
const { meetCode } = require('../src/selectors.js');

test('meetCode reads the code from both call address forms', () => {
  assert.equal(meetCode('/vzc-zjey-fxu'), 'vzc-zjey-fxu');
  assert.equal(meetCode('/meet/vzc-zjey-fxu'), 'vzc-zjey-fxu');
  assert.equal(meetCode('/vzc-zjey-fxu/'), 'vzc-zjey-fxu');
  assert.equal(meetCode('/'), null);
  assert.equal(meetCode('/landing'), null);
  assert.equal(meetCode('/meet/'), null);
});
