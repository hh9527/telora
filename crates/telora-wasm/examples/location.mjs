// ABI 17: independent u32 components, never a packed u64 Number.
export function location(words) {
  return {
    source: words[0],
    start: {line: words[1], offset: words[2]},
    end: {line: words[3], offset: words[4]},
    line: words[1] + 1,
    column: words[2] + 1,
    endLine: words[3] + 1,
    endColumn: words[4] + 1,
  };
}

export function readLocation(word, id) {
  id >>>= 0;
  if (id === 0) return [0, 0, 0, 0, 0];
  const isStatic = (id & 0x80000000) !== 0;
  const descriptor = isStatic ? 20 : 28;
  const index = isStatic ? id & 0x7fffffff : id - 1;
  if (index >= word(descriptor + 4)) throw Error('无效的 LocId');
  const address = word(descriptor) + index * 20;
  if (address + 20 > 2 ** 32) throw Error('位置表地址溢出');
  return Array.from({length: 5}, (_, i) => word(address + i * 4));
}
