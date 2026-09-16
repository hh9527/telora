// Run against the portable-values bundle fixture, including malformed transport.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {load} from './host.mjs';
import {injectBundle} from './bundle.mjs';
const bytes=await readFile(process.argv[2]), module=await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
const section=name=>JSON.parse(new TextDecoder().decode(WebAssembly.Module.customSections(module,name)[0]));
const manifest=section('telora.manifest'), original=section('telora.data');
const session=await load(bytes); session.initialize();
assert.equal(session.eval(),'[{"base":[1,2],"copy":[1,2],"max":9223372036854775807,"number":42},42]');
function customModule(bundle) {
  const leb=n=>{const out=[]; do {let byte=n&127; n=Math.floor(n/128); out.push(byte|(n?128:0));} while(n); return out;};
  const name=new TextEncoder().encode('telora.data'), data=new TextEncoder().encode(JSON.stringify(bundle));
  const payload=[...leb(name.length),...name,...data];
  return new WebAssembly.Module(Uint8Array.from([0,97,115,109,1,0,0,0,0,...leb(payload.length),...payload]));
}
for (const mutate of [
  bundle=>bundle.version=99,
  bundle=>bundle.modules.pop(),
  bundle=>bundle.modules.push(bundle.modules[0]),
  bundle=>bundle.modules[0].source.id=0,
  bundle=>bundle.modules[0].source.id=0x100000000,
  bundle=>bundle.modules[0].format=4,
  bundle=>bundle.modules[0].text='\ud800',
  bundle=>bundle.modules[0].source.name='\ud800',
]) {
  const bundle=structuredClone(original); mutate(bundle);
  let allocated=false;
  assert.throws(()=>injectBundle(customModule(bundle),manifest,{wasm:{'mem-alloc'(){allocated=true; throw Error('unexpected allocation');}}}));
  assert.equal(allocated,false);
}
console.log('Node: bundled data and malformed-bundle rejections passed');
