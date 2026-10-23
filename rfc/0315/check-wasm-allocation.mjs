import fs from 'node:fs';
import assert from 'node:assert/strict';
const path = process.argv[2];
if (!path) throw new Error('usage: node check-wasm-allocation.mjs probe.wasm');
const { instance } = await WebAssembly.instantiate(fs.readFileSync(path));
const e = instance.exports;
e.configure(16 * 1024 * 1024);
const actor = e.allocate(24);
const property = e.allocate(32);
assert.equal(actor % 8, 0);
assert.equal(e.load(property), 0);
e.store(property, 42);
assert.equal(e.owner(property + 31), property);
let plateau;
for (let call = 0; call < 1000; call++) {
  // These are default-allocator addresses, not arena offsets. No reset, image
  // prefix, page free-list or compaction is involved.
  const output = e.allocate(128 * 1024);
  e.store(output + 128 * 1024 - 1, 7);
  assert.equal(e.load(output + 128 * 1024 - 1), 7);
  e.mark(actor + 8);
  e.mark(property + 31);
  e.sweep();
  assert.equal(e.bytes(), 56);
  assert.equal(e.objects(), 2);
  assert.equal(e.load(property), 42);
  if (call === 20) plateau = e.memory.buffer.byteLength;
  if (call > 20) assert.equal(e.memory.buffer.byteLength, plateau);
}
assert.equal(e.set_limit(1), 0);
e.sweep();
assert.equal(e.bytes(), 0);
assert.equal(e.objects(), 0);
console.log(JSON.stringify({calls: 1000, retainedBytes: e.bytes(), memoryPlateau: plateau}));
