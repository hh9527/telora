// Read-only debug transport; never calls language functions or Display providers.
import { location } from './location.mjs';
export function debugReader({manifest, word, payload, text, view}) {
  const repr = pointer => {
    let output = '', bytes = 0, truncated = false;
    const encoder = new TextEncoder();
    const push = value => {
      if (truncated) return;
      for (const ch of value) {
        const width = encoder.encode(ch).length;
        if (bytes + width > 4093) { truncated = true; break; }
        output += ch; bytes += width;
      }
    };
    const quoted = value => {
      push('"');
      for (const ch of value) {
        if (truncated) break;
        const escapes = {'\0': '\\0', '\n': '\\n', '\r': '\\r', '\t': '\\t', '"': '\\"', '\\': '\\\\'};
        push(escapes[ch] ?? (ch.codePointAt(0) < 32 || ch === '\x7f' ? `\\u{${ch.codePointAt(0).toString(16)}}` : ch));
      }
      push('"');
    };
    const value = (pointer, depth = 0) => {
      if (truncated) return;
      const desc = manifest.types[word(pointer + 12)];
      if (!desc || pointer + desc.bytes > view().byteLength) throw Error('无效的 debug 值');
      switch (desc.kind) {
        case 'Int': return push(view().getBigInt64(pointer + 16, true).toString());
        case 'Float': {
          const number = view().getFloat64(pointer + 16, true);
          return push(Number.isNaN(number) ? 'NaN' : !Number.isFinite(number) ? (number < 0 ? '-inf' : 'inf') : Object.is(number, -0) ? '-0.0' : Number.isInteger(number) ? number + '.0' : String(number));
        }
        case 'Bool': return push(word(pointer + 16) ? "'True" : "'False");
        case 'Unit': return push('()');
        case 'String': return quoted(text(pointer));
        case 'Function': return push('<fn>');
        case 'Dyn': return push('<dyn>');
        case 'Metadata': return push(`<TypeId:${word(pointer + 16)}>`);
        case 'Unsupported': return push('<opaque>');
        case 'Bytes': {
          const [base, bytes] = payload(1, word(pointer + 16));
          const start = word(pointer + 20), end = word(pointer + 24);
          if (start > end || end > bytes) throw Error('无效的 debug Bytes');
          push('b"');
          for (let index = start; index < Math.min(end, start + 32); index++) push('\\x' + view().getUint8(base + index).toString(16).padStart(2, '0'));
          if (end - start > 32) push('...');
          return push('"');
        }
      }
      if (depth >= 8) return push('...');
      if (['Option', 'Enum', 'Value'].includes(desc.kind)) {
        const variant = desc.variants[word(pointer + 16)];
        if (!variant) throw Error('无效的 debug variant');
        push("'"); push(variant.name);
        if (variant.ty !== null) {
          push('('); value(variant.boxed ? payload(4, word(pointer + 24))[0] : pointer + 24, depth + 1); push(')');
        }
        return;
      }
      if (desc.kind === 'Newtype') {
        push('('); value(payload(6, word(pointer + 16))[0], depth + 1); return push(')');
      }
      if (desc.kind === 'Array' || desc.kind === 'Dict') {
        const dict = desc.kind === 'Dict';
        const [base, bytes] = payload(3, word(pointer + (dict ? 24 : 16)));
        const start = dict ? 0 : word(pointer + 20), end = word(pointer + (dict ? 20 : 24));
        const stride = manifest.types[desc.arguments[0]].bytes;
        if (start > end || end * stride > bytes || (!stride && end)) throw Error('无效的 debug sequence');
        const keys = dict ? payload(3, word(pointer + 16)) : [0, 0];
        if (dict && end * 32 > keys[1]) throw Error('无效的 debug Dict keys');
        push(dict ? '{' : '[');
        for (let index = start; index < Math.min(end, start + 32) && !truncated; index++) {
          if (index !== start) push(', ');
          if (dict) { push(text(keys[0] + index * 32)); push(': '); }
          value(base + index * stride, depth + 1);
        }
        if (end - start > 32) push(', ...');
        return push(dict ? '}' : ']');
      }
      if (desc.kind === 'Tuple' || desc.kind === 'Record') {
        const record = desc.kind === 'Record', [base, bytes] = payload(2, word(pointer + 16));
        push(record ? '{' : '(');
        for (const [index, field] of desc.fields.slice(0, 32).entries()) {
          if (truncated) break;
          if (field.offset + manifest.types[field.ty].bytes > bytes) throw Error('无效的 debug field');
          if (index) push(', ');
          if (record) { push(field.name); push(': '); }
          value(base + field.offset, depth + 1);
        }
        if (desc.fields.length > 32) push(', ...');
        return push(record ? '}' : ')');
      }
      throw Error('未知的 debug 类型');
    };
    value(pointer);
    return output + (truncated ? '...' : '');
  };
  let cursor = 0;
  return () => {
    const count = word(64 + 13 * 16 + 4), events = [];
    for (let index = cursor; index < count; index++) {
      const [pointer, bytes] = payload(13, index);
      if (bytes !== 8) throw Error('无效的 debug event');
      const site = manifest.debug_sites.find(site => site.node === word(pointer));
      if (!site) throw Error('未知的 debug site');
      const {name, origin, message} = site;
      const loc = location(origin);
      const source = manifest.sources.find(source => source.id === loc.source);
      if (!source) throw Error('debug site 没有来源');
      const module = source.name, line = loc.line;
      events.push({name, repr: repr(word(pointer + 4)), module, line, ...(message === null ? {} : {message})});
    }
    cursor = count;
    return events;
  };
}
