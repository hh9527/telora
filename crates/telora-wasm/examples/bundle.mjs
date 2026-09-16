// Original bytes cross the ABI; Guest owns parsing and Value construction.
import {heapAccess} from './heap.mjs';
export function injectBundle(module, manifest, {wasm}) {
  const sections = WebAssembly.Module.customSections(module, 'telora.data');
  if (!sections.length) return;
  if (sections.length !== 1) throw Error('重复数据包');
  const decoder = new TextDecoder('utf-8', {fatal: true}), encoder = new TextEncoder();
  const bundle = JSON.parse(decoder.decode(sections[0]));
  if (bundle.version !== 2 || !Array.isArray(bundle.modules)) throw Error('不支持的数据包协议');
  const expected = manifest.data_modules.map(item => item.symbol).sort((a,b) => a-b);
  const actual = bundle.modules.map(item => item.symbol).sort((a,b) => a-b);
  if (JSON.stringify(actual) !== JSON.stringify(expected) || new Set(actual).size !== actual.length) throw Error('数据模块清单不匹配');
  const sources = new Map(manifest.sources.map(source => [source.id, source]));
  for (const {source, format, text} of bundle.modules) {
    if (!source || !Number.isInteger(source.id) || source.id <= 0 || source.id > 0xffffffff
        || typeof source.name !== 'string' || !source.name.isWellFormed()
        || ![1,2,3].includes(format) || typeof text !== 'string' || !text.isWellFormed()) throw Error('无效数据来源或格式');
    const previous = sources.get(source.id);
    if (previous && previous.name !== source.name) throw Error('数据来源冲突');
    sources.set(source.id, source);
  }
  const bytes = data => {
    const ptr = wasm['mem-alloc'](data.length, 1) >>> 0;
    new Uint8Array(wasm.memory.buffer, ptr, data.length).set(data);
    return ptr;
  };
  const {word, address} = heapAccess(wasm);
  for (const {symbol, source, format, text} of bundle.modules) {
    // Registration owns a copy; this Host buffer is borrowed only for the call.
    const name = encoder.encode(source.name), namePtr = bytes(name);
    if (!wasm.telora_register_source(source.id, namePtr, name.length)) throw Error('数据来源冲突');
    wasm['mem-free'](namePtr, name.length, 1);
    const input = encoder.encode(text), ptr = bytes(input);
    const packet = wasm.telora_parse_data(ptr, input.length, format, source.id) >>> 0;
    wasm['mem-free'](ptr, input.length, 1);
    const error = word(packet + 12);
    if (error) {
      const start = word(error + 8), length = word(error + 12);
      throw Error(decoder.decode(new Uint8Array(wasm.memory.buffer, address(start), length)));
    }
    const value = wasm.telora_materialize_data(packet, 0);
    if (!wasm.telora_inject_data(symbol, value)) throw Error('数据注入失败');
    if (!manifest.sources.some(item => item.id === source.id)) manifest.sources.push(source);
  }
}
