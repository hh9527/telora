import assert from 'node:assert/strict';
import { location, readLocation } from './location.mjs';

const max = 0xffffffff;
assert.deepEqual(location([max, max, max, max, max]), {
  source: max, start: {line: max, offset: max}, end: {line: max, offset: max},
  line: 2 ** 32, column: 2 ** 32, endLine: 2 ** 32, endColumn: 2 ** 32,
});
const sources = [{id: 1, lines: [[0, 3], [5, 9], [10, 10]]},
  {id: max, lines: [[0, max]]}];
assert.deepEqual(readLocation(sources, [0, 0, 0]), [0, 0, 0, 0, 0]);
assert.deepEqual(readLocation(sources, [1, 2, 8]), [1, 0, 2, 1, 3]);
assert.deepEqual(readLocation(sources, [1, 4, 5]), [1, 0, 3, 1, 0]);
assert.deepEqual(readLocation(sources, [max, max, max]), [max, 0, max, 0, max]);
for (const range of [[1, 8, 7], [1, 0, 11], [0, 1, 1], [2, 0, 0], [1, -1, 0]]) {
  assert.throws(() => readLocation(sources, range));
}
