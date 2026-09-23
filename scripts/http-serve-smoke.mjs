// First run build-run-smoke.mjs; pass its reported root as the argument.
import {spawn, spawnSync} from 'node:child_process';
import {request} from 'node:http';
import {createServer} from 'node:net';
import {join, resolve} from 'node:path';
import {rmSync} from 'node:fs';
import assert from 'node:assert/strict';
const root=process.argv[2];
assert.ok(root,'expected build-run-smoke root');
const dir=join(root,'crlf');
const stdio=spawnSync(resolve('target/release/telora'),['-C',dir,'run','@src/main','--source',`model=${join(dir,'source.json')}`,'--serve','stdio+jsonl://'],{input:'"query"\n"query"\n',encoding:'utf8',timeout:120000});
assert.equal(stdio.status,0,stdio.stderr);
const lines=stdio.stdout.trim().split('\n').map(JSON.parse);
assert.equal(lines.length,2); assert.equal(lines[0].error,false); assert.deepEqual(lines[0],lines[1]);
async function port() {
  const server=createServer();
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  const p=server.address().port;
  await new Promise(r=>server.close(r));
  return p;
}
function send(address,body='"query"',extra={}) {
  return new Promise((resolve,reject)=>{
    const req=request({...address,path:'/transform',method:'POST',...extra},res=>{
      let text=''; res.setEncoding('utf8'); res.on('data',s=>text+=s);
      res.on('end',()=>resolve({status:res.statusCode,value:JSON.parse(text)}));
    });
    req.setTimeout(10000,()=>req.destroy(new Error('request timeout')));
    req.on('error',reject); req.end(body);
  });
}
for(const compiler of [false,true]) for(const unix of [false,true]) {
  if(unix && process.platform==='win32') continue;
  const p=await port(), socket=join(root,`serve-${compiler}.sock`);
  const uri=unix?`http+unix://${socket}`:`http://127.0.0.1:${p}`;
  const address=unix?{socketPath:socket}:{host:'127.0.0.1',port:p};
  const args=compiler?['-C',dir,'--request-fuel','100','run','@src/main']: [join(dir,'original.wasm'),'--request-fuel','1'];
  args.push('--source',`model=${join(dir,'source.json')}`,'--serve',uri);
  const child=spawn(resolve(`target/release/${compiler?'telora':'telora-run'}`),args,{stdio:['ignore','ignore','pipe']});
  let errors='';child.stderr.on('data',s=>errors+=s);
  try {
    let first;
    for(let i=0;i<300;i++) {
      if(child.exitCode!==null) throw new Error(errors);
      try {first=await send(address);break;} catch {await new Promise(r=>setTimeout(r,100));}
    }
    assert.ok(first,errors); assert.equal(first.status,200); assert.equal(first.value.error,false);
    assert.equal((await send(address,'"loop"')).value.error,true);
    assert.deepEqual(await send(address),first);
    assert.equal((await send(address,'null')).value.error,true);
    assert.equal((await send(address,'{')).value.error,true);
    assert.deepEqual(await send(address),first);
    assert.equal((await send(address,'',{method:'GET'})).status,405);
    assert.equal((await send(address,'',{path:'/missing'})).status,404);
    assert.equal((await send(address,'',{headers:{'content-length':'999999999999'}})).status,413);
    const replies=await Promise.all(Array.from({length:8},()=>send(address)));
    for(const reply of replies) assert.deepEqual(reply,first);
  } finally {
    if(child.exitCode===null && child.signalCode===null) {
      const closed=new Promise(r=>child.once('close',r));
      child.kill('SIGTERM');
      await closed;
    }
    if(unix) rmSync(socket,{force:true});
  }
}
console.log(JSON.stringify({passed:true,transports:['http','http+unix'],entries:['telora','telora-run']}));
