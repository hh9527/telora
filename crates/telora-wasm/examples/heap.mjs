// Language references are offsets; Host buffers/source records are raw addresses.
export function heapAccess(wasm) {
  const view = () => new DataView(wasm.memory.buffer);
  const address = reference => wasm.telora_heap_address(reference) >>> 0;
  const word = reference => view().getUint32(address(reference), true);
  const store = (reference, value) => view().setUint32(address(reference), value, true);
  const copy = (to, from, bytes) => wasm.telora_heap_copy(to, from, bytes);
  const content = pointer => {
    const memory = new Uint8Array(wasm.memory.buffer);
    const payload = address(pointer + 16), tag = memory[payload + 15];
    if (tag < 16) return memory.subarray(payload, payload + tag);
    if (tag !== 16) throw Error('无效的 String/Bytes 标签');
    const start = word(pointer + 16), end = word(pointer + 20), raw = word(pointer + 24);
    const base = view().getUint32(48, true), length = view().getUint32(52, true);
    if (raw > start || start > end || end > length) throw Error('无效的内容切片');
    return memory.subarray(base + start, base + end);
  };
  return {view, address, word, store, copy, content};
}
