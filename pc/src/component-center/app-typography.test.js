import test from 'node:test';
import assert from 'node:assert/strict';
import * as typography from './app-typography.js';

test('component labels use one glyph-cell origin for a shared line', () => {
  assert.equal(typeof typography.glyphCellTop, 'function');
  assert.equal(typography.glyphCellTop(20, 24, 16), 24);
  assert.equal(typography.glyphCellTop(20, 18, 18), 20);
});
