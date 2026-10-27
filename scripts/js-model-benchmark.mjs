#!/usr/bin/env node
// Same-request IPC comparison. Both runtimes are dedicated child processes.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {readFileSync,writeFileSync,statSync} from 'node:fs';
import {createInterface} from 'node:readline';
import {ModelProcess} from '../crates/telora-js/host/process.mjs';
const args=Object.fromEntries(process.argv.slice(2).map((v,i,a)=>v.startsWith('--')?[v.slice(2),a[i+1]]:null).filter(Boolean));
if(!args.js||!args.cases||!args.runner)throw Error('Usage: --js MODEL.mjs --cases CASES.json --runner telora-run [--iterations 100] [--report FILE]');
const suite=JSON.parse(readFileSync(args.cases,'utf8')),input=suite.cases[0].input,iterations=Number(args.iterations??100);
assert(Number.isSafeInteger(iterations)&&iterations>1);
const model=new ModelProcess({model:args.js,memoryMiB:Number(args.memory??512)});
const start=performance.now(),info=await model.initialize(),jsColdStartMs=performance.now()-start;
const child=spawn(args.runner,[suite.wasm,'--serve','stdio+jsonl://'],{stdio:['pipe','pipe','pipe']});
let stderr='';child.stderr.on('data',s=>{stderr+=s;});
const reader=createInterface({input:child.stdout}),lines=reader[Symbol.asyncIterator]();
const closed=new Promise(resolve=>child.once('close',resolve));
child.once('error',error=>{stderr+=error.message;reader.close();});
const request=async()=>{const timer=setTimeout(()=>child.kill(),60000);try{child.stdin.write(JSON.stringify({method:suite.domain+'/transform',input})+'\n');const line=await lines.next();assert(!line.done,stderr);return JSON.parse(line.value);}finally{clearTimeout(timer);}};
const js=[],wasm=[],jsExecution=[];
const samples=xs=>{const s=[...xs].sort((a,b)=>a-b);return{median:s[Math.floor(s.length/2)],p95:s[Math.floor(s.length*.95)],min:s[0],max:s.at(-1)};};
let firstWasmMs,firstJsMs,wasmRssKiB,jsRssKiB,jsPeakRssKiB,cgroupPeakBytes;
try {
 let t=performance.now();const expected=await request();firstWasmMs=performance.now()-t;
 t=performance.now();assert.deepEqual(await model.request(suite.domain+'/transform',input),expected);firstJsMs=performance.now()-t;
 for(let i=0;i<iterations;i++){
  t=performance.now();assert.deepEqual(await request(),expected);wasm.push(performance.now()-t);
  t=performance.now();assert.deepEqual(await model.request(suite.domain+'/transform',input),expected);js.push(performance.now()-t);jsExecution.push(model.lastExecutionMs);
 }
 wasmRssKiB=Number(readFileSync(`/proc/${child.pid}/status`,'utf8').match(/^VmRSS:\s+(\d+)/m)?.[1]);
 jsRssKiB=model.stats.memory.rss/1024;jsPeakRssKiB=model.stats.maxRssKiB;
 const group=readFileSync(`/proc/${info.pid}/cgroup`,'utf8').trim().split(':')[2];cgroupPeakBytes=Number(readFileSync(`/sys/fs/cgroup${group}/memory.peak`,'utf8'));
}finally{model.close();child.stdin.end();child.kill();const timer=setTimeout(()=>child.kill('SIGKILL'),5000);await closed;clearTimeout(timer);reader.close();}
const report={domain:suite.domain,iterations,case:suite.cases[0].id,bytes:{js:statSync(args.js).size,wasm:statSync(suite.wasm).size},js:{coldStartMs:jsColdStartMs,loadMs:info.loadMs,initializeMs:info.initializeMs,firstRequestMs:firstJsMs,ipcMs:samples(js),executionMs:samples(jsExecution),rssKiB:jsRssKiB,peakRssKiB:jsPeakRssKiB,cgroupPeakBytes,memoryLimitMiB:info.memoryMiB},wasm:{firstRequestIncludingStartupMs:firstWasmMs,ipcMs:samples(wasm),rssKiB:wasmRssKiB},runtime:{node:process.version,reference:'wasmi via telora-run'}};
if(args.report)writeFileSync(args.report,JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));
