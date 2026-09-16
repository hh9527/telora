// Independent u32 components, never a packed u64 Number.
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

export function readLocation(sources, range) {
  if (!Array.isArray(range) || range.length !== 3 ||
      range.some(n => !Number.isInteger(n) || n < 0 || n > 0xffffffff)) throw Error('无效的来源范围');
  const [id, start, end] = range;
  if (id === 0) {
    if (start || end) throw Error('无效的空来源');
    return [0, 0, 0, 0, 0];
  }
  const source = sources.find(source => source.id === id);
  if (!source?.lines?.length || start > end || end > source.lines.at(-1)[1]) throw Error('来源范围越界');
  const point = byte => {
    let lo = 0, hi = source.lines.length;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (source.lines[mid][0] <= byte) lo = mid + 1; else hi = mid;
    }
    if (!lo) throw Error('无效的来源索引');
    const [start, end] = source.lines[lo - 1];
    return [lo - 1, Math.min(byte, end) - start];
  };
  return [id, ...point(start), ...point(end)];
}
