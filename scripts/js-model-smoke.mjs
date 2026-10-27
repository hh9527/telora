#!/usr/bin/env node
// Compare a generated JS service with its Wasm reference, without deployment.
import assert from 'node:assert/strict';
import {readFileSync, statSync, writeFileSync} from 'node:fs';
import {ModelProcess} from '../crates/telora-js/host/process.mjs';
import {resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {DatabaseSync} from 'node:sqlite';
const args=Object.fromEntries(process.argv.slice(2).map((v,i,a)=>v.startsWith('--')?[v.slice(2),a[i+1]]:null).filter(Boolean));
if(!args.js || !args.cases || !args.runner)throw Error('Usage: --js MODEL.mjs --cases CASES.json --runner telora-run [--report FILE]');
const suite=JSON.parse(readFileSync(args.cases,'utf8'));
const started=performance.now();
const modelProcess=new ModelProcess({model:args.js,memoryMiB:Number(args.memory??512),confinement:'systemd'});
const modelInfo=await modelProcess.initialize();
const coldStartMs=performance.now()-started;
const transform=(method,input)=>modelProcess.request(method,input);
const cached=args.reference?new Map(JSON.parse(readFileSync(args.reference,'utf8')).map(x=>[x.id,x.response])):null;
const child=cached?null:spawn(args.runner,[suite.wasm,'--serve','stdio+jsonl://'],{stdio:['pipe','pipe','pipe']});
let stderr='';child?.stderr.on('data',data=>{stderr+=data;});
const reader=child?createInterface({input:child.stdout}):null,lines=reader?.[Symbol.asyncIterator]();
const closed=child?new Promise(resolve=>child.once('close',resolve)):Promise.resolve();
child?.once('error',e=>{stderr+=e.message;reader?.close();});
const db=suite.db?new DatabaseSync(suite.db,{readOnly:true}):null;
const timings=[],errors=[],rejectionDifferences=[];
if(db)db.function('regexp',{deterministic:true},(pattern,value)=>value===null?0:Number(new RegExp(pattern,'u').test(String(value))));
let accepted=0,rejected=0,sqlQueries=0;
const withoutLocations=x=>Array.isArray(x)?x.map(withoutLocations):x!==null&&typeof x==='object'?Object.fromEntries(Object.entries(x).filter(([k])=>k!=='locs').map(([k,v])=>[k,withoutLocations(v)])):x;
const referenceTimings=[],referenceResponses=[];
try {
 for(const test of suite.cases) {
  const referenceStarted=performance.now();
  let reference;
  if(cached){reference=cached.get(test.id);assert(reference,'missing cached reference '+test.id);}
  else {
    const deadline=setTimeout(()=>child.kill(),60000);
    try{child.stdin.write(JSON.stringify({method:suite.domain+'/transform',input:test.input})+'\n');const next=await lines.next();assert(!next.done,stderr);reference=JSON.parse(next.value);}finally{clearTimeout(deadline);}
  }
  referenceTimings.push(performance.now()-referenceStarted);
  referenceResponses.push({id:test.id,response:reference});
  if(referenceResponses.length%100===0)console.error(`${suite.domain}: ${referenceResponses.length}/${suite.cases.length}`);
  const t=performance.now();let result;
  try {result=await transform(suite.domain+'/transform',test.input);timings.push(performance.now()-t);
   assert.equal(result.error,reference.error,'service error');
   assert.equal(result.ok?.accepted,reference.ok?.accepted,'accepted');
   if(test.expectedAccepted!==undefined)assert.equal(result.ok?.accepted,test.expectedAccepted,'expected acceptance');
   if(reference.ok?.accepted){assert.deepEqual(withoutLocations(result),withoutLocations(reference));accepted++;
    if(db)for(const query of result.ok.queries){db.prepare(query.sql).all(...query.bindings);sqlQueries++;}
   }else{assert.deepEqual(result.ok?.queries,reference.ok?.queries,'rejection has queries');rejected++;
    if(JSON.stringify(result)!==JSON.stringify(reference))rejectionDifferences.push({id:test.id,js:result,wasm:reference});
   }
  }catch(error){errors.push({id:test.id,message:error.message,js:result,wasm:reference});}
 }
} finally {db?.close();modelProcess.close();child?.stdin.end();child?.kill();const timer=setTimeout(()=>child?.kill('SIGKILL'),5000);await closed;clearTimeout(timer);reader?.close();}
const report={reference:cached?'cached-wasm':'live-wasm',wasmTransformMs:cached?null:{first:referenceTimings[0],median:[...referenceTimings.slice(1)].sort((a,b)=>a-b)[Math.floor((referenceTimings.length-1)/2)],max:Math.max(...referenceTimings.slice(1))},domain:suite.domain,cases:suite.cases.length,accepted,rejected,sqlQueries,failures:errors.length,errors,rejectionDiagnosticDifferences:rejectionDifferences.length,rejectionDifferences,bytes:statSync(args.js).size,coldStartMs,loadMs:modelInfo.loadMs,initializeMs:modelInfo.initializeMs,confinement:modelInfo.confinement,memoryLimitMiB:modelInfo.memoryMiB,transformMs:{first:timings[0],median:[...timings].sort((a,b)=>a-b)[Math.floor(timings.length/2)],max:Math.max(...timings)},memory:modelProcess.stats?.memory,maxRssKiB:modelProcess.stats?.maxRssKiB,hostMemory:process.memoryUsage()};
if(args.golden)writeFileSync(args.golden,JSON.stringify(referenceResponses));
if(args.report)writeFileSync(args.report,JSON.stringify(report,null,2));
console.log(JSON.stringify({...report,errors:errors.slice(0,3),rejectionDifferences:rejectionDifferences.slice(0,2)},null,2));
if(errors.length)process.exitCode=1;
