// Run against a CLI-published bundle-data fixture, including malformed transport.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {load} from './host.mjs';
import {injectBundle} from './bundle.mjs';
const bytes=await readFile(process.argv[2]), module=await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
const section=name=>JSON.parse(new TextDecoder().decode(WebAssembly.Module.customSections(module,name)[0]));
const manifest=section('telora.manifest'), original=section('telora.data');
const session=await load(bytes); session.initialize(); assert.equal(session.eval(),'[true,true]');
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
  bundle=>bundle.modules[0].packet.root=0xffffffff,
  bundle=>{const p=bundle.modules[0].packet; p.nodes[p.root].value={Array:[p.root]};},
  bundle=>{const p=bundle.modules[0].packet; p.nodes[p.root].value={Array:[0xffffffff]};},
  bundle=>{const p=bundle.modules[0].packet; p.nodes[p.root].origin=[0xffffffff,0,0];},
  bundle=>{const p=bundle.modules[0].packet; p.nodes[p.root].value={Int:'9223372036854775808'};},
  bundle=>{const p=bundle.modules[0].packet; p.nodes[p.root].value={String:'\ud800'};},
  bundle=>{const p=bundle.modules[0].packet; p.nodes[p.root].value={Object:[{name:'z',origin:p.nodes[p.root].origin,value:0},{name:'a',origin:p.nodes[p.root].origin,value:0}]};},
]) {
  const bundle=structuredClone(original); mutate(bundle);
  let allocated=false;
  assert.throws(()=>injectBundle(customModule(bundle),manifest,{allocate(){allocated=true; throw Error('unexpected allocation');}}));
  assert.equal(allocated,false);
}
console.log('Node: bundled data and ten malformed-bundle rejections passed');
