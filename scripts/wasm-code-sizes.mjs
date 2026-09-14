// Inspect a linked, zero-import Telora artifact without executing it.
import { readFileSync } from 'node:fs';

if (process.argv.length !== 3) throw Error('usage: node scripts/wasm-code-sizes.mjs artifact.wasm');
const bytes = readFileSync(process.argv[2]);
if (!bytes.subarray(0, 8).equals(Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]))) {
  throw Error('invalid Wasm header');
}
let offset = 8, codeBytes = 0;
const sizes = [], names = new Map();
function uint() {
  let value = 0;
  for (let shift = 0; shift < 35; shift += 7) {
    if (offset >= bytes.length) throw Error('truncated integer');
    const byte = bytes[offset++];
    value += (byte & 127) * 2 ** shift;
    if (!(byte & 128)) return value;
  }
  throw Error('invalid integer');
}
function string() {
  const length = uint(), end = offset + length;
  if (end > bytes.length) throw Error('truncated string');
  const value = bytes.subarray(offset, end).toString('utf8');
  offset = end;
  return value;
}
while (offset < bytes.length) {
  const start = offset, id = bytes[offset++], length = uint(), end = offset + length;
  if (end > bytes.length) throw Error('truncated section');
  if (id === 2 && uint() !== 0) throw Error('expected a linked zero-import artifact');
  if (id === 10) {
    codeBytes = end - start;
    const count = uint();
    for (let i = 0; i < count; i++) {
      const size = uint();
      sizes.push(size);
      offset += size;
    }
  }
  if (id === 0 && string() === 'name') {
    while (offset < end) {
      const kind = bytes[offset++], size = uint(), subEnd = offset + size;
      if (subEnd > end) throw Error('truncated name subsection');
      if (kind === 1) {
        const count = uint();
        for (let i = 0; i < count; i++) { const index = uint(); names.set(index, string()); }
      }
      offset = subEnd;
    }
  }
  if (offset > end) throw Error('section overflow');
  offset = end;
}
const functions = sizes.map((size, index) => {
  const name = names.get(index) ?? '<unnamed>';
  const labels = Object.fromEntries([...name.matchAll(/\|(module|owner|role|hir|loc|instance)=([^|]*)/g)].map(m => [m[1], m[2]]));
  return { index, bytes: size, name, ...labels };
});
function group(key) {
  const groups = new Map();
  for (const fn of functions) {
    const label = key(fn), row = groups.get(label) ?? { name: label, functions: 0, bytes: 0 };
    row.functions++; row.bytes += fn.bytes; groups.set(label, row);
  }
  return [...groups.values()].sort((a, b) => b.bytes - a.bytes || a.name.localeCompare(b.name));
}
console.log(JSON.stringify({
  artifact_bytes: bytes.length, code_section_bytes: codeBytes,
  function_body_bytes: sizes.reduce((a, b) => a + b, 0),
  by_module: group(fn => fn.module ?? '<runtime-or-entry>'),
  by_role: group(fn => fn.role?.split('[')[0] ?? '<runtime-or-entry>'),
  by_owner: group(fn => fn.module ? `${fn.module}::${fn.owner}` : '<runtime-or-entry>'),
  functions: functions.sort((a, b) => b.bytes - a.bytes || a.index - b.index),
}, null, 2));
