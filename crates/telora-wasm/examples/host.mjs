// External transport only. Language functions, heap allocation and initialization run in Wasm.
export async function load(bytes) {
  const module = await WebAssembly.compile(bytes);
  const sections = WebAssembly.Module.customSections(module, 'telora.manifest');
  if (sections.length !== 1) throw Error('缺少或重复的 Telora manifest');
  const manifest = JSON.parse(new TextDecoder().decode(sections[0]));
  if (manifest.abi !== 1) throw Error('不支持的产物 ABI');
  const { exports: wasm } = await WebAssembly.instantiate(module, {});
  const view = () => new DataView(wasm.memory.buffer);
  const word = address => view().getUint32(address, true);
  const store = (address, value) => view().setUint32(address, value, true);
  const allocate = length => wasm.telora_alloc(length) >>> 0;
  const copy = (destination, source, length) => {
    new Uint8Array(wasm.memory.buffer).copyWithin(destination, source, source + length);
  };
  const push = (table, pointer, length) => wasm.telora_table_push(64 + table * 16, pointer, length) >>> 0;
  const payload = (table, id) => {
    const descriptor = 64 + table * 16;
    if (id >= word(descriptor + 4)) throw Error('无效的 HeapId');
    const slot = word(descriptor) + id * 8;
    return [word(slot), word(slot + 4)];
  };
  const text = pointer => {
    const memory = new Uint8Array(wasm.memory.buffer);
    let bytes;
    if (memory[pointer + 16] === 0) {
      const length = memory[pointer + 17];
      if (length > 14) throw Error('无效的 inline String');
      bytes = memory.subarray(pointer + 18, pointer + 18 + length);
    } else {
      const [base, length] = payload(0, word(pointer + 20));
      const start = word(pointer + 24), end = word(pointer + 28);
      if (start > end || end > length) throw Error('无效的 String slice');
      bytes = memory.subarray(base + start, base + end);
    }
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  };
  const json = (pointer, type, depth = 0) => {
    if (depth > 512) throw Error('输出嵌套过深');
    if (word(pointer + 12) !== type) throw Error('输出类型与封闭签名不同');
    const desc = manifest.types[type];
    switch (desc.kind) {
      case 'Unit': return 'null';
      case 'Int': return view().getBigInt64(pointer + 16, true).toString();
      case 'Bool': return word(pointer + 16) ? 'true' : 'false';
      case 'Float': {
        const value = view().getFloat64(pointer + 16, true);
        if (!Number.isFinite(value)) throw Error('无法输出非有限 Float');
        return JSON.stringify(value);
      }
      case 'String': return JSON.stringify(text(pointer));
      case 'Array': {
        const [base, bytes] = payload(3, word(pointer + 16));
        const start = word(pointer + 20), end = word(pointer + 24);
        const element = desc.arguments[0], stride = manifest.types[element].bytes;
        if (start > end || end * stride > bytes || (!stride && end)) throw Error('无效的 Array slice');
        const items = [];
        for (let i = start; i < end; i++) items.push(json(base + i * stride, element, depth + 1));
        return '[' + items.join(',') + ']';
      }
      case 'Tuple':
      case 'Record': {
        const [base, bytes] = payload(2, word(pointer + 16));
        const items = desc.fields.map(field => {
          if (field.offset + manifest.types[field.ty].bytes > bytes) throw Error('字段越界');
          const item = json(base + field.offset, field.ty, depth + 1);
          return desc.kind === 'Tuple' ? item : JSON.stringify(field.name) + ':' + item;
        });
        return desc.kind === 'Tuple' ? '[' + items.join(',') + ']' : '{' + items.join(',') + '}';
      }
      case 'Dict': {
        const [keys, keyBytes] = payload(3, word(pointer + 16));
        const [values, valueBytes] = payload(3, word(pointer + 24));
        const length = word(pointer + 20), type = desc.arguments[0], stride = manifest.types[type].bytes;
        if (length * 32 !== keyBytes || length * stride !== valueBytes) throw Error('字典列长度不一致');
        const items = [];
        for (let i = 0; i < length; i++) items.push(JSON.stringify(text(keys + i * 32)) + ':' + json(values + i * stride, type, depth + 1));
        return '{' + items.join(',') + '}';
      }
      default: throw Error('尚不支持此类型的浏览器输出');
    }
  };
  const input = (type, value, depth = 0) => {
    if (depth > 512) throw Error('输入嵌套过深');
    const desc = manifest.types[type], pointer = allocate(desc.bytes);
    store(pointer + 12, type);
    switch (desc.kind) {
      case 'Unit': if (value !== null) throw Error('需要 Unit'); break;
      case 'Int': {
        if (typeof value !== 'bigint' && !Number.isSafeInteger(value)) throw Error('Int 输入需要安全整数或 BigInt');
        const integer = BigInt(value);
        if (integer < -(1n << 63n) || integer >= (1n << 63n)) throw Error('Int 输入越界');
        view().setBigInt64(pointer + 16, integer, true); break;
      }
      case 'Float': if (typeof value !== 'number' || !Number.isFinite(value)) throw Error('需要 Float');
        view().setFloat64(pointer + 16, value, true); break;
      case 'Bool': if (typeof value !== 'boolean') throw Error('需要 Bool'); store(pointer + 16, Number(value)); break;
      case 'String': {
        if (typeof value !== 'string') throw Error('需要 String');
        const bytes = new TextEncoder().encode(value);
        if (bytes.length <= 14) {
          const memory = new Uint8Array(wasm.memory.buffer);
          memory[pointer + 17] = bytes.length; memory.set(bytes, pointer + 18);
        } else {
          const data = allocate(bytes.length);
          new Uint8Array(wasm.memory.buffer).set(bytes, data);
          const id = push(0, data, bytes.length);
          store(pointer + 16, 1); store(pointer + 20, id); store(pointer + 28, bytes.length);
        }
        break;
      }
      case 'Array': {
        if (!Array.isArray(value)) throw Error('需要 Array');
        const type = desc.arguments[0], stride = manifest.types[type].bytes;
        const data = allocate(value.length * stride);
        value.forEach((item, index) => copy(data + index * stride, input(type, item, depth + 1), stride));
        const id = push(3, data, value.length * stride);
        store(pointer + 16, id); store(pointer + 24, value.length); break;
      }
      case 'Tuple':
      case 'Record': {
        if (!value || (desc.kind === 'Tuple' ? !Array.isArray(value) : typeof value !== 'object' || Array.isArray(value))
            || Object.keys(value).length !== desc.fields.length) throw Error('输入形状不匹配');
        const bytes = desc.fields.reduce((end, field) => Math.max(end, field.offset + manifest.types[field.ty].bytes), 0);
        const data = allocate(bytes);
        desc.fields.forEach((field, index) => {
          const key = desc.kind === 'Tuple' ? index : field.name;
          if (!Object.hasOwn(value, key)) throw Error('缺少输入字段');
          copy(data + field.offset, input(field.ty, value[key], depth + 1), manifest.types[field.ty].bytes);
        });
        store(pointer + 16, push(2, data, bytes)); break;
      }
      default: throw Error('尚不支持此类型的浏览器输入');
    }
    return pointer;
  };
  const failure = () => {
    const pointer = wasm.telora_error.value >>> 0;
    if (!pointer) return Error('会话未初始化或已失败');
    const source = word(pointer), start = word(pointer + 4), end = word(pointer + 8), code = word(pointer + 12);
    const file = manifest.sources.find(file => file.id === source)?.name ?? '<unknown>';
    const loc = manifest.locations.find(loc => loc.source === source && loc.start === start && loc.end === end);
    const message = ['执行失败', 'integer arithmetic overflowed', 'integer division by zero', 'initialization dependency cycle', 'array index out of bounds', 'dictionary key is absent'][code] ?? '执行失败';
    return Error(`${file}:${loc?.line ?? start}:${loc?.column ?? end}: ${message}`);
  };
  return {
    initialize() { if (!wasm.telora_initialize()) throw failure(); },
    eval() {
      const pointer = wasm.telora_entry() >>> 0;
      if (!pointer) throw failure();
      return json(pointer, manifest.entry_type);
    },
    call(arguments_) {
      const closure = wasm.telora_entry() >>> 0;
      if (!closure) throw failure();
      const desc = manifest.types[manifest.entry_type];
      if (desc.kind !== 'Function' || desc.arguments.length !== arguments_.length + 1) throw Error('调用参数与封闭签名不同');
      const args = allocate(arguments_.length * 4);
      arguments_.forEach((value, index) => store(args + index * 4, input(desc.arguments[index], value)));
      const result = wasm.telora_invoke(closure, args) >>> 0;
      if (!result) throw failure();
      return json(result, desc.arguments.at(-1));
    },
  };
}
