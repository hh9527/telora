// End-to-end publication tests use external Telora fixtures, not embedded Rust strings.
// cargo build --release -p telora -p telora-run && node scripts/build-run-smoke.mjs
import {mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
const compiler=resolve(process.env.TELORA_BIN ?? 'target/release/telora');
const runner=resolve(process.env.TELORA_RUN_BIN ?? 'target/release/telora-run');
const root=mkdtempSync(join(tmpdir(),'telora-build-run-'));
const service=readFileSync('crates/telora-run/tests/fixtures/service.telora','utf8');
const constants='{"constant":\n 42\n}\n';
const data='{\n"name":"retained source"\n}\n';
function call(binary,args,input,success=true) {
  const result=spawnSync(binary,args,{input,encoding:'utf8',timeout:120000,maxBuffer:8*1024*1024});
  assert.equal(result.error,undefined,String(result.error));
  assert.equal(result.status===0,success,JSON.stringify({args,status:result.status,stderr:result.stderr}));
  return result;
}
const originals=[];
const eols=[['lf','\n'],['crlf','\r\n'],['cr','\r']];
for (const [label,eol] of eols) {
  const dir=join(root,label); mkdirSync(join(dir,'src'),{recursive:true});
  writeFileSync(join(dir,'telora-config.json'),JSON.stringify({version:1,members:['.']}));
  writeFileSync(join(dir,'telora-crate.json'),JSON.stringify({name:'build-fixture',modules:['@src/main','@src/constants.json'],dependencies:[]}));
  writeFileSync(join(dir,'src/main.telora'),service.replaceAll('\n',eol));
  writeFileSync(join(dir,'src/constants.json'),constants.replaceAll('\n',eol));
  writeFileSync(join(dir,'source.json'),data.replaceAll('\n',eol));
  call(compiler,['-C',dir,'lock']);
  const original=join(dir,'original.wasm');
  call(compiler,['-C',dir,'build','@src/main','-o',original]);
  originals.push(readFileSync(original));
  const normal=call(runner,[original,'--source',`model=${join(dir,'source.json')}`],'"query"');
  assert.deepEqual(JSON.parse(normal.stdout),[{name:'retained source'},{constant:42},'query',true,false]);
  const diagnostics=call(runner,[original,'--source',`model=${join(dir,'source.json')}`],'null',false);
  assert.ok(diagnostics.stderr.includes('@service/model'));
  assert.ok(diagnostics.stderr.includes('service diagnostic'));
  const args=[original,'--bind','stdio+jsonl://','--with-fuel','1','--with-memory-limit','4'];
  args.push('--source',`model=${join(dir,'source.json')}`);
  const result=call(runner,args,'"query"\n"loop"\n"query"\n"grow"\n"query"\nnull\n"query"\n');
  const replies=result.stdout.trim().split('\n').map(JSON.parse);
  assert.equal(replies.length,7);
  assert.deepEqual(replies.map(r=>r.error),[false,true,false,true,false,true,false]);
  for (const index of [2,4,6]) assert.deepEqual(replies[index],replies[0]);
  const memoryArgs=[original,'--bind','stdio+jsonl://','--with-fuel','1000','--with-memory-limit','2'];
  memoryArgs.push('--source',`model=${join(dir,'source.json')}`);
  const memoryResult=call(runner,memoryArgs,'"query"\n"grow"\n"query"\n');
  const memoryReplies=memoryResult.stdout.trim().split('\n').map(JSON.parse);
  assert.deepEqual(memoryReplies.map(r=>r.error),[false,true,false]);
  assert.match(JSON.stringify(memoryReplies[1]),/growth|growing memory/);
  assert.deepEqual(memoryReplies[0],memoryReplies[2]);
}
for (let i=1;i<originals.length;i++) {
  assert.deepEqual(originals[i],originals[0],'ordinary build differs by EOL');
}
const dir=join(root,'lf');
writeFileSync(join(dir,'src/main.telora'),readFileSync('crates/telora-run/tests/fixtures/failed-init.telora'));
call(compiler,['-C',dir,'build','@src/main','-o',join(dir,'failed-ordinary.wasm')]);
const failed=call(runner,[join(dir,'failed-ordinary.wasm')],'"query"',false);
const errors=failed.stderr.trim().split('\n').map(JSON.parse);
assert.ok(errors.some(e=>Array.isArray(e.labels) && e.labels.length>0),'initialization diagnostic lost its locations');
const before=readFileSync(join(dir,'original.wasm'));
writeFileSync(join(dir,'src/main.telora'),readFileSync('crates/telora-run/tests/fixtures/invalid.telora'));
call(compiler,['-C',dir,'build','@src/main','-o',join(dir,'original.wasm')],undefined,false);
assert.deepEqual(readFileSync(join(dir,'original.wasm')),before,'failed compilation replaced output');
call(compiler,['-C',dir,'build','@src/main','-o',join(dir,'failed.wasm')],undefined,false);
assert.equal(existsSync(join(dir,'failed.wasm')),false);
const invalid=Buffer.from(originals[0]); invalid[0]=1; writeFileSync(join(dir,'invalid.wasm'),invalid);
call(runner,[join(dir,'invalid.wasm')],'"query"',false);
console.log(JSON.stringify({passed:true,root,ordinary_bytes:originals[0].length}));
