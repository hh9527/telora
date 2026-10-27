// Private runtime helpers linked into the single generated module.
const none = () => variant(0), some = x => variant(1,x), err = x => variant(0,x), ok = x => variant(1,x);
const immutable = x => Object.freeze(x);
const dictKeys = x => Object.keys(x).sort();
const natives = {
  '1:equal': (s,a,b) => equal(a,b),
  '5:length': (s,a) => BigInt(a.length),
  '5:get': (s,a,i) => i>=0n && i<BigInt(a.length) ? some(a[Number(i)]) : none(),
  '5:enumerate': (s,a) => immutable(a.map((v,i)=>immutable([BigInt(i),v]))),
  '5:push': (s,a,v) => immutable([...a,v]),
  '5:concat': (s,a) => immutable(a.flat()),
  '5:zip': (s,a,b) => a.length===b.length ? some(immutable(a.map((v,i)=>immutable([v,b[i]])))) : none(),
  '5:map': (s,a,f) => immutable(a.map(v=>f(v))),
  '5:filter': (s,a,f) => immutable(a.filter(v=>f(v))),
  '5:flat_map': (s,a,f) => immutable(a.flatMap(v=>f(v))),
  '5:fold': (s,a,z,f) => a.reduce((r,v)=>f(r,v),z),
  '5:fold_control': (s,a,z,f) => { for(const v of a){const r=f(z,v); if(r.tag===1)return r; z=r.value;} return variant(0,z); },
  '5:any': (s,a,f) => a.some(v=>f(v)),
  '5:all': (s,a,f) => a.every(v=>f(v)),
  '5:find': (s,a,f) => {for(const v of a)if(f(v))return some(v); return none();},
  '6:keys': (s,a) => immutable(dictKeys(a)),
  '6:values': (s,a) => immutable(dictKeys(a).map(k=>a[k])),
  '6:pairs': (s,a) => immutable(dictKeys(a).map(k=>immutable([k,a[k]]))),
  '6:from_pairs': (s,a) => immutable(Object.fromEntries(a)),
  '6:merge': (s,a,b) => immutable({...a,...b}),
  '6:map_values': (s,a,f) => immutable(Object.fromEntries(dictKeys(a).map(k=>[k,f(a[k])]))),
  '6:filter': (s,a,f) => immutable(Object.fromEntries(dictKeys(a).filter(k=>f(a[k])).map(k=>[k,a[k]]))),
  '6:fold': (s,a,z,f) => dictKeys(a).reduce((r,k)=>f(r,k,a[k]),z),
  '6:get': (s,a,k) => Object.hasOwn(a,k)?some(a[k]):none(),
  '7:length': (s,a) => BigInt(new TextEncoder().encode(a).length),
  '7:join': (s,a,sep) => a.join(sep),
  '7:join_lines': (s,a) => a.join('\n'),
  '7:split': (s,a,sep) => immutable(sep===''?['',...a,'']:a.split(sep)),
  '7:lines': (s,a) => immutable(a.split(/\r?\n/).filter((v,i,x)=>i<x.length-1||v!=='')),
  '7:starts_with': (s,a,b) => a.startsWith(b),
  '7:ends_with': (s,a,b) => a.endsWith(b),
  '7:contains': (s,a,b) => a.includes(b),
  '7:replace': (s,a,b,c) => a.replaceAll(b,c),
  '7:indent': (s,a,n) => a.split('\n').map(x=>' '.repeat(Number(n))+x).join('\n'),
  '7:ensure_trailing_newline': (s,a) => a.endsWith('\n')?a:a+'\n',
  '7:trim_margin': (s,a,m) => a.split('\n').map(x=>{const i=x.indexOf(m);return i<0?x:x.slice(i+m.length);}).join('\n'),
};
const native = (key,signature) => {
  const impl = natives[key];
  if(!impl) throw Error(`Telora JS native not implemented: ${key}`);
  return (...args) => impl(signature,...args);
};
