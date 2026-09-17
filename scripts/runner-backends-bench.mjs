// Compare identical Wasm artifacts, including first response and repeated requests.
// Requires /usr/bin/time. No compiler or persistent engine cache is used.
import {parseArgs} from 'node:util';
import {spawn} from 'node:child_process';
import {readFileSync,writeFileSync,statSync} from 'node:fs';
import assert from 'node:assert/strict';
const {values:a}=parseArgs({options:{
  wasmi:{type:'string'},wasmtime:{type:'string'},ordinary:{type:'string'},snapshot:{type:'string'},
  input:{type:'string'},output:{type:'string'},runs:{type:'string',default:'3'},
  requests:{type:'string',default:'31'},modes:{type:'string',default:'wasmi,pulley,pulley-speed,cranelift-none,cranelift-speed,cranelift-speed-and-size'},
}});
for(const key of ['wasmi','wasmtime','ordinary','snapshot','input','output']) assert.ok(a[key],`missing --${key}`);
const input=JSON.stringify(JSON.parse(readFileSync(a.input,'utf8')))+'\n';
const count=Number(a.requests), runs=Number(a.runs);
assert.ok(Number.isInteger(count)&&count>=6&&Number.isInteger(runs)&&runs>0);
const median=xs=>xs.sort((x,y)=>x-y)[Math.floor(xs.length/2)];
const rows=[]; let expected;
async function measure(mode,kind) {
  const binary=mode==='wasmi'?a.wasmi:a.wasmtime;
  const started=performance.now();
  const child=spawn('/usr/bin/time',['-f','RSS_KIB=%M',binary,a[kind],'--mode',mode,'--serve','--report-timings'],{stdio:['pipe','pipe','pipe']});
  let stdout='',stderr='',first_response_ms;
  child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
  child.stdout.on('data',chunk=>{
    stdout+=chunk;
    if(first_response_ms===undefined&&stdout.includes('\n')) first_response_ms=performance.now()-started;
  });
  child.stderr.on('data',chunk=>stderr+=chunk);
  child.stdin.end(input.repeat(count));
  const timer=setTimeout(()=>child.kill('SIGKILL'),120000);
  const status=await new Promise((resolve,reject)=>{
    child.on('error',reject); child.on('close',(code,signal)=>resolve({code,signal}));
  });
  clearTimeout(timer);
  const process_ms=performance.now()-started;
  assert.equal(status.code,0,`${mode}/${kind}: ${stderr}`);
  const responses=stdout.trim().split('\n').map(JSON.parse);
  assert.equal(responses.length,count);
  for(const r of responses) {
    assert.equal(r.error,false,JSON.stringify(r));
    if(expected===undefined) expected=r;
    assert.deepEqual(r,expected);
  }
  const lines=stderr.trim().split('\n');
  const records=lines.filter(l=>l.startsWith('{')).map(JSON.parse).filter(r=>r.code==='execution-timings');
  assert.equal(records.length,count);
  const first=records[0];
  assert.equal(first.mode,mode);
  const row={mode,kind,first_response_ms,process_ms,read_ms:first.read_ms,...first.timings,
    steady_request_ms:median(records.slice(5).map(r=>r.timings.request_ms)),
    steady_reset_ms:median(records.slice(5).map(r=>r.timings.reset_ms)),
    rss_kib:Number(lines.find(l=>l.startsWith('RSS_KIB=')).slice(8)),wasm_bytes:statSync(a[kind]).size};
  rows.push(row);
  console.log(JSON.stringify({mode,kind,first_ms:row.first_response_ms,compile_ms:row.module_ms,steady_ms:row.steady_request_ms}));
}
const modes=a.modes.split(',');
for(let i=0;i<runs;i++) {
  const order=i%2?[...modes].reverse():modes;
  for(const mode of order) for(const kind of i%2?['snapshot','ordinary']:['ordinary','snapshot']) await measure(mode,kind);
}
const summary=[];
for(const mode of modes) for(const kind of ['ordinary','snapshot']) {
  const rs=rows.filter(r=>r.mode===mode&&r.kind===kind);
  summary.push({mode,kind,...Object.fromEntries(Object.keys(rs[0]).filter(k=>!['mode','kind'].includes(k)).map(k=>[k,median(rs.map(r=>r[k]))]))});
}
writeFileSync(a.output,JSON.stringify({runs,requests:count,rows,summary,expected},null,2));
console.log(JSON.stringify({summary,output:a.output},null,2));
