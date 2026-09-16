import assert from 'node:assert/strict';
import { location, readLocation } from './location.mjs';

const max = 0xffffffff;
assert.deepEqual(location([max, max, max, max, max]), {
  source: max, start: {line: max, offset: max}, end: {line: max, offset: max},
  line: 2 ** 32, column: 2 ** 32, endLine: 2 ** 32, endColumn: 2 ** 32,
});
const words = new Map([[20, 64], [24, 1], [28, 84], [32, 1]]);
[1, 70000, 1 << 25, 70001, 5, 2, 0, 0, 1, 0].forEach((n, i) => words.set(64 + i * 4, n));
const word = offset => {
  assert.ok(words.has(offset));
  return words.get(offset);
};
assert.deepEqual(readLocation(word, 0), [0, 0, 0, 0, 0]);
assert.deepEqual(readLocation(word, 0x80000000), [1, 70000, 1 << 25, 70001, 5]);
assert.deepEqual(readLocation(word, 1), [2, 0, 0, 1, 0]);
assert.throws(() => readLocation(word, 2), /LocId/);
assert.throws(() => readLocation(word, 0x80000001), /LocId/);
