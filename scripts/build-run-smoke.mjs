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
const originals=[],snapshots=[];
for (const [label,eol] of [['lf','\n'],['crlf','\r\n'],['cr','\r']]) {
  const dir=join(root,label); mkdirSync(join(dir,'src'),{recursive:true});
  writeFileSync(join(dir,'telora-config.json'),JSON.stringify({version:1,members:['.']}));
  writeFileSync(join(dir,'telora-crate.json'),JSON.stringify({name:'snapshot-fixture',modules:['@src/main','@src/constants.json'],dependencies:[]}));
  writeFileSync(join(dir,'src/main.telora'),service.replaceAll('\n',eol));
  writeFileSync(join(dir,'src/constants.json'),constants.replaceAll('\n',eol));
  writeFileSync(join(dir,'source.json'),data.replaceAll('\n',eol));
  call(compiler,['-C',dir,'lock']);
  const original=join(dir,'original.wasm'),snapshot=join(dir,'snapshot.wasm');
  call(compiler,['-C',dir,'build','@src/main','-o',original]);
  call(compiler,['-C',dir,'build','@src/main','--snapshot','--source',`model=${join(dir,'source.json')}`,'-o',snapshot]);
  originals.push(readFileSync(original)); snapshots.push(readFileSync(snapshot));
  const normal=call(runner,[original,'--source',`model=${join(dir,'source.json')}`],'"query"');
  const restored=call(runner,[snapshot],'"query"');
  assert.deepEqual(JSON.parse(restored.stdout),[{name:'retained source'},{constant:42},'query',true,false]);
  assert.equal(restored.stdout,normal.stdout);
  const diagnostics=call(runner,[snapshot],'null',false);
  assert.ok(diagnostics.stderr.includes('@service/model'));
  assert.ok(diagnostics.stderr.includes('snapshot diagnostic'));
  call(runner,[snapshot,'--source',`model=${join(dir,'source.json')}`],'"query"',false);
  for (const artifact of [original,snapshot]) {
    const args=[artifact,'--serve','--with-fuel','1','--with-memory-limit','4'];
    if (artifact===original) args.push('--source',`model=${join(dir,'source.json')}`);
    const result=call(runner,args,'"query"\n"loop"\n"query"\n"grow"\n"query"\nnull\n"query"\n');
    const replies=result.stdout.trim().split('\n').map(JSON.parse);
    assert.equal(replies.length,7);
    assert.deepEqual(replies.map(r=>r.error),[false,true,false,true,false,true,false]);
    for (const index of [2,4,6]) assert.deepEqual(replies[index],replies[0]);
    const memoryArgs=[artifact,'--serve','--with-fuel','1000','--with-memory-limit','2'];
    if (artifact===original) memoryArgs.push('--source',`model=${join(dir,'source.json')}`);
    const memoryResult=call(runner,memoryArgs,'"query"\n"grow"\n"query"\n');
    const memoryReplies=memoryResult.stdout.trim().split('\n').map(JSON.parse);
    assert.deepEqual(memoryReplies.map(r=>r.error),[false,true,false]);
    assert.ok(JSON.stringify(memoryReplies[1]).includes('growth'));
    assert.deepEqual(memoryReplies[0],memoryReplies[2]);
  }
}
for (let i=1;i<3;i++) {
  assert.deepEqual(originals[i],originals[0],'ordinary build differs by EOL');
  assert.deepEqual(snapshots[i],snapshots[0],'snapshot differs by EOL');
}
const dir=join(root,'lf');
const before=readFileSync(join(dir,'snapshot.wasm'));
writeFileSync(join(dir,'src/main.telora'),readFileSync('crates/telora-run/tests/fixtures/failed-init.telora'));
call(compiler,['-C',dir,'build','@src/main','--snapshot','-o',join(dir,'snapshot.wasm')],undefined,false);
assert.deepEqual(readFileSync(join(dir,'snapshot.wasm')),before,'failed initialization replaced output');
call(compiler,['-C',dir,'build','@src/main','--snapshot','-o',join(dir,'failed.wasm')],undefined,false);
assert.equal(existsSync(join(dir,'failed.wasm')),false);
call(compiler,['-C',dir,'build','@src/main','-o',join(dir,'failed-ordinary.wasm')]);
const failed=call(runner,[join(dir,'failed-ordinary.wasm')],'"query"',false);
const errors=failed.stderr.trim().split('\n').map(JSON.parse);
assert.ok(errors.some(e=>Array.isArray(e.labels) && e.labels.length>0),'initialization diagnostic lost its locations');
const invalid=Buffer.from(snapshots[0]); invalid[0]=1; writeFileSync(join(dir,'invalid.wasm'),invalid);
call(runner,[join(dir,'invalid.wasm')],'"query"',false);
console.log(JSON.stringify({passed:true,root,ordinary_bytes:originals[0].length,snapshot_bytes:snapshots[0].length}));
