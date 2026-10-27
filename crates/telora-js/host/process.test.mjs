import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,rmSync,readFileSync,mkdirSync} from 'node:fs';
import {join} from 'node:path';
import {homedir} from 'node:os';
import {ModelProcess} from './process.mjs';
const base=process.env.TMPDIR||join(homedir(),'.cache','tmp');mkdirSync(base,{recursive:true});
function fixture(body){const dir=mkdtempSync(join(base,'telora-js-process-'));const model=join(dir,'model.mjs');writeFileSync(model,body);return{model,close:()=>rmSync(dir,{recursive:true,force:true})};}
const hard=process.env.TELORA_JS_HARD_LIMIT_TESTS==='1';
test('IPC, request timeout and recreation preserve the host',async()=>{
 const f=fixture('export function createService(){return (method,input)=>{if(input.hang){while(true){}}return {method,input};};}');
 const p=new ModelProcess({model:f.model,confinement:hard?'systemd':'unconfined-test',requestTimeoutMs:300});
 try{
  const first=await p.initialize();assert.deepEqual(await p.request('echo',{value:42}),{method:'echo',input:{value:42}});
  await assert.rejects(p.request('echo',{hang:true}),/request timeout/);
  assert.deepEqual(await p.request('echo',{value:43}),{method:'echo',input:{value:43}});
  const second=await p.initialize();assert.notEqual(second.pid,first.pid);
  if(hard){const group=readFileSync(`/proc/${second.pid}/cgroup`,'utf8').trim().split(':')[2];assert.equal(readFileSync(`/sys/fs/cgroup${group}/memory.max`,'utf8').trim(),String(512*1024*1024));}
 }finally{p.close();f.close();}
});
test('initialization timeout terminates a non-returning model',async()=>{
 const f=fixture('export function createService(){while(true){}}');
 const p=new ModelProcess({model:f.model,confinement:hard?'systemd':'unconfined-test',initializationTimeoutMs:150});
 try{await assert.rejects(p.initialize(),/initialization timeout/);}finally{p.close();f.close();}
});
test('oversized model responses are rejected at the IPC boundary',async()=>{
 const f=fixture('export function createService(){return()=>"x".repeat(100000);}');
 const p=new ModelProcess({model:f.model,confinement:hard?'systemd':'unconfined-test',maxResponseBytes:4096});
 try{await assert.rejects(p.request('echo',{}),/response byte quota/);}finally{p.close();f.close();}
});
test('cgroup hard memory limit kills allocations outside the V8 old-space heap',{skip:!hard},async()=>{
 const f=fixture('export function createService(){return (method,input)=>{if(input.allocate){const arrays=[];while(true){arrays.push(new Uint8Array(16*1024*1024).fill(1));}}return "host-survived";};}');
 const p=new ModelProcess({model:f.model,memoryMiB:128,requestTimeoutMs:10000});
 try{await assert.rejects(p.request('allocate',{allocate:true}),/process exited|EPIPE/);assert.equal(await p.request('echo',{}),'host-survived');}finally{p.close();f.close();}
});
