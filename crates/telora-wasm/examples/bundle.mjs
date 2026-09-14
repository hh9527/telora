// External data transport only: the artifact supplies all concrete layouts.
import { location } from './location.mjs';
export function injectBundle(module, manifest, rt) {
  const sections = WebAssembly.Module.customSections(module, 'telora.data');
  if (!sections.length) return;
  if (sections.length !== 1) throw Error('重复数据包');
  const bundle = JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(sections[0]));
  if (bundle.version !== 1 || !Array.isArray(bundle.modules)) throw Error('不支持的数据包协议');
  const expected = manifest.data_modules.map(item => item.symbol).sort((a,b) => a-b);
  const actual = bundle.modules.map(item => item.symbol).sort((a,b) => a-b);
  if (JSON.stringify(actual) !== JSON.stringify(expected) || new Set(actual).size !== actual.length) throw Error('数据模块清单不匹配');
  const {allocate, store, copy, push, input, wasm} = rt;
  const origin = loc => {
    if (!Array.isArray(loc) || loc.length !== 3 || loc.some(n => !Number.isInteger(n) || n < 0 || n > 0xffffffff)
        || location(loc).start > location(loc).end || !manifest.sources.some(source => source.id === location(loc).source)) throw Error('无效数据来源');
  };
  const utf8 = text => new TextEncoder().encode(text);
  const compare = (a,b) => {
    const x = utf8(a), y = utf8(b);
    for (let i=0; i<Math.min(x.length,y.length); i++) if (x[i] !== y[i]) return x[i]-y[i];
    return x.length-y.length;
  };
  const tagged = value => typeof value === 'string' ? [value, undefined] :
    value && typeof value === 'object' && Object.keys(value).length === 1 ? Object.entries(value)[0] : (() => {throw Error('无效数据节点');})();
  // Validate the complete bundle before the first allocation or injection.
  for (const {packet} of bundle.modules) {
    if (!packet || !Array.isArray(packet.nodes)) throw Error('无效数据图');
    const validId = id => Number.isInteger(id) && id >= 0 && id < packet.nodes.length;
    if (!validId(packet.root)) throw Error('无效数据根');
    const edges = packet.nodes.map(node => {
      origin(node.origin);
      const [kind, value] = tagged(node.value);
      let children = [];
      switch (kind) {
        case 'Null': if (value !== undefined) throw Error('无效 Null'); break;
        case 'Bool': if (typeof value !== 'boolean') throw Error('无效 Bool'); break;
        case 'Int': {
          if (typeof value !== 'string' || !/^[+-]?\d+$/.test(value)) throw Error('无效 Int');
          const n = BigInt(value); if (n < -(1n<<63n) || n >= (1n<<63n)) throw Error('Int 越界'); break;
        }
        case 'Float': if (typeof value !== 'number' || !Number.isFinite(value)) throw Error('无效 Float'); break;
        case 'String': if (typeof value !== 'string' || !value.isWellFormed()) throw Error('无效 String'); break;
        case 'Bytes': if (!Array.isArray(value) || value.some(n => !Number.isInteger(n) || n<0 || n>255)) throw Error('无效 Bytes'); break;
        case 'Temporal': if (!value || !['LocalDate','LocalTime','LocalDateTime','OffsetDateTime'].includes(value.variant) || typeof value.value !== 'string' || !value.value.isWellFormed()) throw Error('无效时间'); break;
        case 'Array': if (!Array.isArray(value)) throw Error('无效 Array'); children = value; break;
        case 'Object':
          if (!Array.isArray(value)) throw Error('无效 Object');
          value.forEach((field,i) => {
            origin(field.origin);
            if (typeof field.name !== 'string' || !field.name.isWellFormed() || i && compare(value[i-1].name, field.name) >= 0) throw Error('字段必须有序且唯一');
          });
          children = value.map(field => field.value); break;
        default: throw Error('未知数据节点');
      }
      if (children.some(id => !validId(id))) throw Error('数据引用越界');
      return children;
    });
    const state = new Uint8Array(packet.nodes.length);
    const visit = (id, depth) => {
      if (depth > 512) throw Error('数据嵌套过深');
      if (state[id] === 1) throw Error('数据图存在循环');
      if (state[id] === 2) return;
      state[id] = 1;
      edges[id].forEach(child => visit(child, depth+1));
      state[id] = 2;
    };
    packet.nodes.forEach((_,id) => visit(id,0));
  }
  const locate = (pointer, loc) => loc.forEach((word,i) => store(pointer+i*4,word));
  const fresh = type => {const pointer=allocate(manifest.types[type].bytes); store(pointer+12,type); return pointer;};
  for (const {symbol, packet} of bundle.modules) {
    const cache = new Map(), valueType = manifest.value_type, desc = manifest.types[valueType];
    const materialize = (id, depth=0) => {
      if (depth > 512) throw Error('数据嵌套过深');
      if (cache.has(id)) return cache.get(id);
      const node=packet.nodes[id], [kind,value]=tagged(node.value);
      const tag=kind==='Null'?'None':kind==='Bool'?(value?'True':'False'):kind==='Temporal'?value.variant:kind;
      const index=desc.variants.findIndex(branch=>branch.name===tag), branch=desc.variants[index];
      if (!branch) throw Error('数据分支缺少封闭布局');
      let payload;
      if (branch.ty !== null) {
        const type=branch.ty;
        if (kind==='Array') {
          payload=fresh(type);
          const stride=desc.bytes, data=allocate(value.length*stride);
          value.forEach((child,i)=>copy(data+i*stride,materialize(child,depth+1),stride));
          store(payload+16,push(3,data,value.length*stride)); store(payload+24,value.length);
        } else if (kind==='Object') {
          payload=fresh(type);
          const string=manifest.types.findIndex(type=>type.kind==='String'), keyWidth=manifest.types[string].bytes;
          const keys=allocate(value.length*keyWidth), values=allocate(value.length*desc.bytes);
          value.forEach((field,i)=>{
            const key=input(string,field.name); locate(key,field.origin);
            copy(keys+i*keyWidth,key,keyWidth); copy(values+i*desc.bytes,materialize(field.value,depth+1),desc.bytes);
          });
          store(payload+16,push(3,keys,value.length*keyWidth)); store(payload+20,value.length);
          store(payload+24,push(3,values,value.length*desc.bytes));
        } else payload=input(type,kind==='Int'?BigInt(value):kind==='Temporal'?value.value:value);
        locate(payload,node.origin);
      }
      const pointer=fresh(valueType); locate(pointer,node.origin); store(pointer+16,index);
      if (payload !== undefined) {
        const width=manifest.types[branch.ty].bytes;
        if (branch.boxed) store(pointer+24,push(4,payload,width)); else copy(pointer+24,payload,width);
      }
      cache.set(id,pointer); return pointer;
    };
    if (!wasm.telora_inject_data(symbol,materialize(packet.root))) throw Error('数据注入失败');
  }
}
