// Build graph descriptors are private to the generated module.
const stageFactories=Object.create(null), stageDescriptors=new WeakMap();
const stageWeakEntries=new WeakMap();
function stageDescribe(value,description){if(typeof stageRuntimeOnly==='undefined'||!stageRuntimeOnly)stageDescriptors.set(value,description);return value;}
function stageClosure(id,captures){
  let callable;
  const fn=(...args)=>(callable??=stageFactories[id](captures()))(...args);
  return stageDescribe(fn,()=>({kind:'closure',id,captures:captures()}));
}
function stageBuiltin(kind,args){
  const fn=kind==='variant'?x=>variant(args[0],x):kind==='check'?x=>check(args[0],'type',x):()=>property(...args);
  return stageDescribe(fn,()=>({kind:'builtin',name:kind,args}));
}
function stagePack(root){
  const nodes=[],seen=new Map();
  function ref(value){
    if(value===undefined)return [];
    if(typeof value==='bigint')return ['i',String(value)];
    if(typeof value==='number' && (!Number.isFinite(value)||Object.is(value,-0)))return ['n',Object.is(value,-0)?'-0':String(value)];
    if(value===null || !['object','function'].includes(typeof value))return value;
    if(seen.has(value))return [seen.get(value)];
    const index=nodes.length;seen.set(value,index);nodes.push(null);
    let node;
    if(stageDescriptors.has(value))node=[2,ref(stageDescriptors.get(value)())];
    else if(typeof value==='function')throw Error('Build result contains an unregistered closure');
    else if(value instanceof RegExp)node=[5,value.source,value.flags,value.lastIndex];
    else if(value instanceof Map)node=[3,[...value].map(([k,v])=>[ref(k),ref(v)])];
    else if(value instanceof WeakMap){const entries=stageWeakEntries.get(value);if(!entries)throw Error('Untracked build WeakMap');node=[4,entries.map(([k,v])=>[ref(k),ref(v)])];}
    else if(Array.isArray(value))node=[0,Object.isFrozen(value),value.map(ref)];
    else if(Object.getPrototypeOf(value)===Object.prototype || Object.getPrototypeOf(value)===null)node=[1,Object.isFrozen(value),Object.getPrototypeOf(value)===null,Object.entries(value).map(([k,v])=>[k,ref(v)])];
    else throw Error('Unsupported build object '+value.constructor?.name);
    nodes[index]=node;return [index];
  }
  const entry=ref(root);return {schema:'telora.js.build/v1',entry,nodes};
}
// These helpers deliberately live outside graph decoding: restored functions must
// not retain the decoder's image, node table, or unrelated prepared objects.
function stageForward(){const cell={target:null};return {cell,fn:(...args)=>cell.target(...args)};}
function stageRestoreDescriptor(d){
  if(d.kind==='closure')return stageClosure(d.id,()=>d.captures);
  if(d.kind==='native')return native(d.key,d.signature);
  if(d.kind==='nativeResult')return native(d.key,d.signature)(...d.args);
  if(d.kind==='builtin')return stageBuiltin(d.name,d.args);
  if(d.kind==='memo')return d.state===2?()=>d.value:memo(d.calc);
  throw Error('Unsupported build descriptor '+d.kind);
}
function stageUnpack(image){
  if(image.schema!=='telora.js.build/v1')throw Error('Unsupported JS build image');
  const values=new Array(image.nodes.length),state=new Uint8Array(image.nodes.length);
  function read(r){
    if(!Array.isArray(r))return r;
    if(r.length===0)return undefined;
    if(r[0]==='i')return BigInt(r[1]);
    if(r[0]==='n')return r[1]==='-0'?-0:Number(r[1]);
    const i=r[0],n=image.nodes[i];
    if(state[i])return values[i];
    state[i]=1;
    if(n[0]===2){
      const {cell,fn}=stageForward();values[i]=fn;
      cell.target=stageRestoreDescriptor(read(n[1]));
    } else if(n[0]===5){
      const v=values[i]=new RegExp(n[1],n[2]);v.lastIndex=n[3];
    } else if(n[0]===3||n[0]===4){
      const v=values[i]=n[0]===3?new Map():new WeakMap();
      for(const [k,x] of n[1])v.set(read(k),read(x));
    } else if(n[0]===0){
      const v=values[i]=[];for(const x of n[2])v.push(read(x));if(n[1])Object.freeze(v);
    } else {
      const v=values[i]=Object.create(n[2]?null:Object.prototype);
      for(const [k,x] of n[3])Object.defineProperty(v,k,{value:read(x),enumerable:true,writable:true,configurable:true});
      if(n[1])Object.freeze(v);
    }
    state[i]=2;return values[i];
  }
  return read(image.entry);
}
