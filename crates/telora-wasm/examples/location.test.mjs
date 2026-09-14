import assert from 'node:assert/strict';
import { location } from './location.mjs';

// The coordinates use 40 bits, below Number's 53-bit exact integer limit.
const max = 2 ** 40 - 1;
assert.deepEqual(location([0xffffffff, 0xffffffff, 0xffffffff]), {
  source: 65535, start: max, end: max,
  line: 65536, column: 16777216, endLine: 65536, endColumn: 16777216,
});
const across = location([0x01000001, 0xff000000, 0]);
assert.equal(across.source, 1);
assert.equal(across.line, 256);
assert.equal(across.endLine, 257);
assert.equal(across.end - across.start, 2 ** 24);
