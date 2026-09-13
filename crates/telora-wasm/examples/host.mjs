// External transport only. Language functions, heap allocation and initialization run in Wasm.
export async function load(bytes) {
  const module = await WebAssembly.compile(bytes);
  const sections = WebAssembly.Module.customSections(module, 'telora.manifest');
  if (sections.length !== 1) throw Error('缺少或重复的 Telora manifest');
  const manifest = JSON.parse(new TextDecoder().decode(sections[0]));
  if (manifest.abi !== 3) throw Error('不支持的产物 ABI');
  const { exports: wasm } = await WebAssembly.instantiate(module, {});
  const view = () => new DataView(wasm.memory.buffer);
  const word = address => view().getUint32(address, true);
  const store = (address, value) => view().setUint32(address, value, true);
  const allocate = length => wasm.telora_alloc(length) >>> 0;
  for (const source of manifest.sources) {
    const name = new TextEncoder().encode(source.name);
    const pointer = allocate(name.length);
    new Uint8Array(wasm.memory.buffer, pointer, name.length).set(name);
    if (!wasm.telora_register_source(source.id, pointer, name.length)) throw Error('来源身份冲突');
  }
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
      case 'Value':
      case 'Option':
      case 'Enum': {
        const branch = desc.variants[word(pointer + 16)];
        if (!branch) throw Error('无效的 enum tag');
        if (desc.kind === 'Value' && branch.name === 'Bytes') throw Error('Value.Bytes cannot be emitted as semantic JSON');
        let value = null;
        if (branch.ty !== null) {
          const address = branch.boxed ? payload(4, word(pointer + 24))[0] : pointer + 24;
          value = json(address, branch.ty, depth + 1);
        }
        if (desc.kind === 'Value') {
          if (branch.name === 'None') return 'null';
          if (branch.name === 'True') return 'true';
          if (branch.name === 'False') return 'false';
          if (value === null) throw Error('缺少 Value payload');
          return value;
        }
        if (desc.kind === 'Option') return value ?? 'null';
        return value === null ? JSON.stringify(branch.name) : '{' + JSON.stringify(branch.name) + ':' + value + '}';
      }
      case 'Newtype': {
        const field = desc.fields[0];
        if (!field) throw Error('缺少 newtype 布局');
        return json(payload(6, word(pointer + 16))[0], field.ty, depth + 1);
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
      case 'Dict': {
        if (!value || typeof value !== 'object' || Array.isArray(value)) throw Error('需要 Dict');
        const string = manifest.types.findIndex(type => type.kind === 'String');
        const entries = Object.entries(value).sort(([a], [b]) => {
          const x = new TextEncoder().encode(a), y = new TextEncoder().encode(b);
          for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] - y[i];
          return x.length - y.length;
        });
        const element = desc.arguments[0], stride = manifest.types[element].bytes;
        const keys = allocate(entries.length * 32), values = allocate(entries.length * stride);
        entries.forEach(([key, value], i) => {
          copy(keys + i * 32, input(string, key, depth + 1), 32);
          copy(values + i * stride, input(element, value, depth + 1), stride);
        });
        store(pointer + 16, push(3, keys, entries.length * 32));
        store(pointer + 20, entries.length);
        store(pointer + 24, push(3, values, entries.length * stride)); break;
      }
      case 'Value':
      case 'Option':
      case 'Enum': {
        let name, item;
        if (desc.kind === 'Value') {
          name = value === null ? 'None' : typeof value === 'boolean' ? (value ? 'True' : 'False')
            : typeof value === 'bigint' || Number.isInteger(value) ? 'Int' : typeof value === 'number' ? 'Float'
            : typeof value === 'string' ? 'String' : Array.isArray(value) ? 'Array' : 'Object';
          item = value;
        } else if (desc.kind === 'Option') { name = value === null ? 'None' : 'Some'; item = value; }
        else if (typeof value === 'string') name = value;
        else if (value && typeof value === 'object' && Object.keys(value).length === 1) [name, item] = Object.entries(value)[0];
        else throw Error('需要 enum 名字或单项 payload 对象');
        const index = desc.variants.findIndex(variant => variant.name === name), branch = desc.variants[index];
        if (!branch || (desc.kind === 'Enum' && (branch.ty !== null) !== (item !== undefined))) throw Error('enum 输入不匹配');
        store(pointer + 16, index);
        if (branch.ty !== null) {
          const payload = input(branch.ty, item, depth + 1), width = manifest.types[branch.ty].bytes;
          if (branch.boxed) store(pointer + 24, push(4, payload, width));
          else copy(pointer + 24, payload, width);
        }
        break;
      }
      case 'Newtype': {
        const ty = desc.fields[0].ty, payload = input(ty, value, depth + 1);
        store(pointer + 16, push(6, payload, manifest.types[ty].bytes)); break;
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
    const message = code === 9 ? text(word(pointer + 16)) : errorMessage(code);
    return Error(`${file}:${loc?.line ?? start}:${loc?.column ?? end}: ${message}`);
  };
  const errorMessage = code => ['执行失败', 'integer arithmetic overflowed', 'integer division by zero', 'initialization dependency cycle', 'array index out of bounds', 'dictionary key is absent', 'property query failed', 'pattern match failed', 'data module has not been injected before initialization'][code] ?? '执行失败';
  const diagnostics = () => {
    const result = [];
    const count = word(64 + 8 * 16 + 4);
    for (let i = 0; i < count; i++) {
      const [pointer, bytes] = payload(8, i);
      if (bytes !== 32) throw Error('无效诊断记录');
      const origin = [word(pointer), word(pointer + 4), word(pointer + 8)], code = word(pointer + 12);
      const message = code === 9 ? text(word(pointer + 16)) : errorMessage(code);
      const base = word(pointer + 20), length = word(pointer + 24), subjects = [];
      for (let index = 0; index < length; index++) {
        const address = base + index * 12, subject = [word(address), word(address + 4), word(address + 8)];
        if (subject[0] && !subjects.some(prior => prior.every((value, j) => value === subject[j]))) subjects.push(subject);
      }
      const source = manifest.sources.find(source => source.id === origin[0])?.name ?? '<unknown>';
      const position = manifest.locations.find(loc => loc.source === origin[0] && loc.start === origin[1] && loc.end === origin[2]);
      result.push({severity: word(pointer + 28) ? 'warning' : 'error', message, source, origin, subjects, line: position?.line, column: position?.column});
    }
    return result;
  };
  const field = (pointer, ty, name) => {
    const desc = manifest.types[ty], field = desc.fields.find(field => field.name === name);
    if (desc.kind !== 'Record' || !field || word(pointer + 12) !== ty) throw Error('入口 record 契约不匹配');
    const [base, bytes] = payload(2, word(pointer + 16));
    if (field.offset + manifest.types[field.ty].bytes > bytes) throw Error('入口字段越界');
    return [base + field.offset, field.ty];
  };
  const invoke = (closure, type, arguments_) => {
    const desc = manifest.types[type];
    if (desc.kind !== 'Function' || desc.arguments.length !== arguments_.length + 1) throw Error('调用参数与封闭签名不同');
    const args = allocate(arguments_.length * 4);
    arguments_.forEach((value, index) => store(args + index * 4, input(desc.arguments[index], value)));
    const result = wasm.telora_invoke(closure, args) >>> 0;
    if (!result) throw failure();
    return json(result, desc.arguments.at(-1));
  };
  return {
    diagnostics,
    injectData(name, value) {
      const module = manifest.data_modules.find(module => module.name === name);
      if (!module) throw Error('数据模块不在产物中');
      if (!wasm.telora_inject_data(module.symbol, input(module.ty, value))) throw Error('数据必须在初始化前恰好注入一次');
    },
    initialize() { if (!wasm.telora_initialize()) throw failure(); },
    eval() {
      const pointer = wasm.telora_entry() >>> 0;
      if (!pointer) throw failure();
      return json(pointer, manifest.entry_type);
    },
    call(arguments_) {
      const closure = wasm.telora_entry() >>> 0;
      if (!closure) throw failure();
      return invoke(closure, manifest.entry_type, arguments_);
    },
    evalWith(context) {
      if (manifest.entry_type !== manifest.eval_type) throw Error('入口需要 std/entry.Eval');
      const root = wasm.telora_entry() >>> 0;
      if (!root) throw failure();
      const [config, configType] = field(root, manifest.entry_type, 'config');
      const declared = JSON.parse(json(config, configType));
      for (const [name, inputName] of [['sources', 'sources'], ['envs', 'env']]) {
        const names = declared[name].toSorted();
        if (names.some((value, i) => !value || value === names[i - 1])) throw Error('配置需要不重复的非空名字');
        if (JSON.stringify(names) !== JSON.stringify(Object.keys(context[inputName]).sort())) throw Error('输入名字与声明不同');
      }
      if (!declared.args && context.args.length) throw Error('此入口不接收参数');
      const [closure, type] = field(root, manifest.entry_type, 'evaluate');
      if (manifest.types[type].arguments.at(-1) !== manifest.value_type) throw Error('入口结果需要 std/value.Value');
      return invoke(closure, type, [context]);
    },
  };
}
