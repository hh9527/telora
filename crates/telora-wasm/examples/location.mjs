// ABI 14: u16 source + two u40 positions, each line:u16 / UTF-8 column:u24.
export function location(words) {
  const start = words[1] + ((words[0] >>> 16) & 255) * 2 ** 32;
  const end = words[2] + (words[0] >>> 24) * 2 ** 32;
  return {
    source: words[0] & 65535,
    start, end,
    line: Math.floor(start / 2 ** 24) + 1,
    column: start % 2 ** 24 + 1,
    endLine: Math.floor(end / 2 ** 24) + 1,
    endColumn: end % 2 ** 24 + 1,
  };
}
