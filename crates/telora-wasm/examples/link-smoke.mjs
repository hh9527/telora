// Run against the artifact linked from link-object and rust-rt-probe.
import { readFile } from "node:fs/promises";
import assert from "node:assert/strict";

const module = await WebAssembly.compile(await readFile(process.argv[2]));
assert.deepEqual(WebAssembly.Module.imports(module), []);
const instance = await WebAssembly.instantiate(module);
const initialBytes = instance.exports.memory.buffer.byteLength;
assert.equal(instance.exports.answer(), 42n);
for (let index = 0; index < 10_000; index++) {
  assert.equal(instance.exports.array_answer(), 12n);
}
assert.ok(instance.exports.memory.buffer.byteLength > initialBytes);
// A second instance has independent runtime allocation state.
const second = await WebAssembly.instantiate(module);
assert.equal(second.exports.memory.buffer.byteLength, initialBytes);
assert.equal(second.exports.array_answer(), 12n);
console.log("Rust RT static link: direct call, indirect callback, heap growth, instance isolation passed");
