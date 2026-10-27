// Trusted host boundary. Model JS is imported exclusively by a child process.
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
let serial=0;
export class ModelProcess {
  constructor({model,memoryMiB=512,initializationTimeoutMs=15000,requestTimeoutMs=5000,maxResponseBytes=16*1024*1024,maxRequestBytes=1024*1024,confinement='systemd'}={}){
    if(!model)throw Error('model path required');
    for(const [name,value] of Object.entries({memoryMiB,initializationTimeoutMs,requestTimeoutMs,maxResponseBytes,maxRequestBytes}))if(!Number.isSafeInteger(value)||value<1)throw Error('invalid '+name);
    if(!['systemd','unconfined-test'].includes(confinement))throw Error('unsupported process confinement');
    Object.assign(this,{model:resolve(model),memoryMiB,initializationTimeoutMs,requestTimeoutMs,maxResponseBytes,maxRequestBytes,confinement});
    this.closed=false;
  }
  async initialize(){
    if(this.closed)throw Error('model process is closed');
    if(this.ready)return this.ready;
    const unit=`telora-js-${process.pid}-${++serial}`;
    const nodeArgs=[`--max-old-space-size=${Math.max(32,Math.floor(this.memoryMiB*.6))}`,'--max-semi-space-size=8',fileURLToPath(new URL('./worker.mjs',import.meta.url)),this.model];
    const command=this.confinement==='systemd'?'systemd-run':process.execPath;
    const args=this.confinement==='systemd'?['--user','--scope','--quiet','--collect',`--unit=${unit}`,'-p',`MemoryMax=${this.memoryMiB}M`,'-p','MemorySwapMax=0','-p','TasksMax=32','--',process.execPath,...nodeArgs]:nodeArgs;
    const generation={unit,child:spawn(command,args,{stdio:['pipe','pipe','pipe']}),buffer:Buffer.alloc(0),stderr:'',pending:null,dead:false};
    this.generation=generation;
    generation.child.stderr.on('data',chunk=>{generation.stderr=(generation.stderr+chunk.toString()).slice(-8192);});
    generation.child.stdout.on('data',chunk=>{
      generation.buffer=Buffer.concat([generation.buffer,chunk]);
      if(generation.buffer.length>this.maxResponseBytes){this.fail(generation,Error('model response byte quota exceeded'));return;}
      let end;
      while((end=generation.buffer.indexOf(10))!==-1){const line=generation.buffer.subarray(0,end);generation.buffer=generation.buffer.subarray(end+1);
        try{const value=JSON.parse(line.toString());if(!generation.pending)throw Error('unsolicited model response');generation.pending.resolve(value);}catch(error){this.fail(generation,error);return;}
      }
    });
    generation.child.once('error',error=>this.fail(generation,error));
    generation.child.stdin.on('error',error=>this.fail(generation,error));
    generation.child.once('exit',(code,signal)=>this.fail(generation,Error(`model process exited (${code??signal}): ${generation.stderr}`)));
    this.ready=this.wait(generation,this.initializationTimeoutMs,'initialization').then(value=>{
      if(value.ready!==true)throw Error('invalid model ready response');
      this.stats=value.stats;
      return {pid:value.pid,loadMs:value.loadMs,initializeMs:value.initializeMs,confinement:this.confinement,memoryMiB:this.memoryMiB};
    }).catch(error=>{this.fail(generation,error);throw error;});
    return this.ready;
  }
  wait(generation,timeout,phase){
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>this.fail(generation,Error(`model ${phase} timeout (${timeout} ms)`)),timeout);
      generation.pending={resolve:value=>{clearTimeout(timer);generation.pending=null;resolve(value);},reject:error=>{clearTimeout(timer);generation.pending=null;reject(error);}};
    });
  }
  async request(method,input){
    if(this.active)throw Error('model requests must be sequential');
    this.active=true;
    try{
      await this.initialize();
      const generation=this.generation,id=++serial;
      const line=JSON.stringify({id,method,input},(_,v)=>typeof v==='bigint'?JSON.rawJSON(v.toString()):v)+'\n';
      if(Buffer.byteLength(line)>this.maxRequestBytes)throw Error('model request byte quota exceeded');
      const response=this.wait(generation,this.requestTimeoutMs,'request');
      generation.child.stdin.write(line);
      const result=await response;
      if(result.id!==id){this.fail(generation,Error('model response id mismatch'));throw Error('model response id mismatch');}
      this.stats=result.stats;
      this.lastExecutionMs=result.transformMs;
      return result.response;
    } finally{this.active=false;}
  }
  fail(generation,error){
    if(generation.dead)return;
    generation.dead=true;
    generation.pending?.reject(error);
    if(this.generation===generation){this.ready=null;this.generation=null;}
    if(this.confinement==='systemd'){
      const killer=spawn('systemctl',['--user','kill','--kill-whom=all',generation.unit+'.scope'],{stdio:'ignore'});
      killer.on('error',()=>{});
    }
    generation.child.kill('SIGKILL');
  }
  close(){this.closed=true;if(this.generation)this.fail(this.generation,Error('model process closed'));}
}
