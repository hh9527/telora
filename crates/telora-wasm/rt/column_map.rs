//! Private MapMut: append-only key/value columns and a sorted u32 index.
//! Input values remain rooted by the generated caller. Comparisons happen before
//! publishing new objects; no mutable state escapes into the language.
use crate::{
    abi::*,
    heap, telora_alloc,
    values::{array_base, word},
};
use alloc::vec::Vec;
use telora_wasm_shared::column_map::Request;
struct MapMut {
    keys: Vec<u32>,
    values: Vec<u32>,
    index: Vec<u32>,
}
unsafe fn copy(dst: u32, src: u32, bytes: u32) {
    unsafe {
        core::ptr::copy_nonoverlapping(heap::ptr::<u8>(src), heap::ptr::<u8>(dst), bytes as usize)
    }
}
unsafe fn record(value: u32) -> u32 {
    unsafe {
        if word(value, DATA) == INLINE_FIELDS {
            value + INLINE_DATA
        } else {
            word(value, DATA) + 8
        }
    }
}
struct Columns {
    d: Request,
    failed: bool,
}
impl Columns {
    unsafe fn compare(&mut self, a: u32, b: u32) -> i64 {
        unsafe {
            match self.d.order {
                1 => {
                    let a = heap::read::<i64>(a + 8);
                    let b = heap::read::<i64>(b + 8);
                    (a > b) as i64 - (a < b) as i64
                }
                2 => crate::values::telora_string_compare(a, b) as i64,
                _ => {
                    if self.failed {
                        return 0;
                    }
                    let args = [a, b];
                    let mut out = [0u64; 2];
                    if crate::values::telora_invoke(
                        self.d.comparator,
                        args.as_ptr() as u32,
                        out.as_mut_ptr() as u32,
                    ) == 0
                    {
                        self.failed = true;
                    }
                    out[1] as i64
                }
            }
        }
    }
    unsafe fn lower(&mut self, b: &MapMut, key: u32) -> (usize, bool) {
        unsafe {
            let mut lo = 0;
            let mut hi = b.index.len();
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let c = self.compare(b.keys[b.index[mid] as usize], key);
                if self.failed {
                    return (0, false);
                }
                if c < 0 { lo = mid + 1 } else { hi = mid }
            }
            let found = lo < b.index.len() && self.compare(b.keys[b.index[lo] as usize], key) == 0;
            (lo, found)
        }
    }
    unsafe fn put(&mut self, b: &mut MapMut, key: u32, value: u32) {
        unsafe {
            let (at, found) = self.lower(b, key);
            if self.failed {
                return;
            }
            if found {
                b.values[b.index[at] as usize] = value;
            } else {
                let id = u32::try_from(b.keys.len()).expect("Map length overflow");
                b.keys.push(key);
                b.values.push(value);
                b.index.insert(at, id);
            }
        }
    }
    unsafe fn load(&self, input: u32) -> MapMut {
        unsafe {
            let n = word(input, DATA + 4);
            let keys = if n == 0 {
                0
            } else {
                word(word(input, DATA), 4)
            };
            let vals = if n == 0 {
                0
            } else {
                word(word(input, DATA + 8), 4)
            };
            MapMut {
                keys: (0..n).map(|i| keys + i * self.d.key_width).collect(),
                values: (0..n).map(|i| vals + i * self.d.value_width).collect(),
                index: (0..n).collect(),
            }
        }
    }
    unsafe fn column(&self, ty: u32, width: u32, items: impl Iterator<Item = u32>, n: u32) -> u32 {
        unsafe {
            let bytes = n
                .checked_mul(width)
                .and_then(|v| v.checked_add(16))
                .expect("Map column overflow");
            let out = telora_alloc(bytes);
            heap::write(out, ty);
            heap::write(out + 4, out + 16);
            heap::write(out + 8, n);
            heap::write(out + 12, n);
            for (i, item) in items.enumerate() {
                copy(out + 16 + i as u32 * width, item, width);
            }
            out
        }
    }
    unsafe fn finish(&self, b: MapMut) -> u32 {
        unsafe {
            let n = b.index.len() as u32;
            let keys = self.column(
                self.d.key_type,
                self.d.key_width,
                b.index.iter().map(|i| b.keys[*i as usize]),
                n,
            );
            let vals = self.column(
                self.d.value_type,
                self.d.value_width,
                b.index.iter().map(|i| b.values[*i as usize]),
                n,
            );
            let out = telora_alloc(24);
            heap::write(out + 8, keys);
            heap::write(out + 12, n);
            heap::write(out + 16, vals);
            out
        }
    }
    unsafe fn pair(&self, key: u32, value: u32) -> u32 {
        unsafe {
            let out = telora_alloc(self.d.pair_width);
            let fields = if self.d.pair_inline != 0 {
                heap::write(out + 8, INLINE_FIELDS);
                out + INLINE_DATA
            } else {
                let fields = telora_alloc(8 + self.d.pair_bytes);
                heap::write(fields, self.d.pair_type);
                heap::write(fields + 4, self.d.pair_bytes);
                heap::write(out + 8, fields);
                fields + 8
            };
            copy(fields + self.d.key_offset, key, self.d.key_width);
            copy(fields + self.d.value_offset, value, self.d.value_width);
            out
        }
    }
    unsafe fn run(&mut self) -> u32 {
        unsafe {
            if self.d.operation == 20 {
                let input = self.d.input;
                let start = word(input, DATA + 4);
                let end = word(input, DATA + 8);
                let base = array_base(input);
                let mut b = MapMut {
                    keys: Vec::new(),
                    values: Vec::new(),
                    index: Vec::new(),
                };
                for i in start..end {
                    let fields = record(base + i * self.d.pair_width);
                    self.put(
                        &mut b,
                        fields + self.d.key_offset,
                        fields + self.d.value_offset,
                    );
                    if self.failed {
                        return 0;
                    }
                }
                return self.finish(b);
            }
            if self.d.operation == 21 {
                let n = word(self.d.input, DATA + 4);
                if n == 0 {
                    return 0;
                }
                let keys = word(word(self.d.input, DATA), 4);
                let values = word(word(self.d.input, DATA + 8), 4);
                let mut lo = 0;
                let mut hi = n;
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    let order = self.compare(keys + mid * self.d.key_width, self.d.key);
                    if self.failed {
                        return 0;
                    }
                    if order == 0 {
                        return values + mid * self.d.value_width;
                    }
                    if order < 0 { lo = mid + 1 } else { hi = mid }
                }
                return 0;
            }
            let mut b = self.load(self.d.input);
            match self.d.operation {
                22 => {
                    self.put(&mut b, self.d.key, self.d.value);
                    if self.failed { 0 } else { self.finish(b) }
                }
                23 => {
                    let (at, found) = self.lower(&b, self.d.key);
                    if self.failed {
                        return 0;
                    }
                    if found {
                        b.index.remove(at);
                    }
                    self.finish(b)
                }
                24 => {
                    let right = self.load(self.d.key);
                    for i in right.index {
                        self.put(&mut b, right.keys[i as usize], right.values[i as usize]);
                        if self.failed {
                            return 0;
                        }
                    }
                    self.finish(b)
                }
                25 => {
                    let n = b.index.len() as u32;
                    let pairs: Vec<u32> = b
                        .index
                        .iter()
                        .map(|i| self.pair(b.keys[*i as usize], b.values[*i as usize]))
                        .collect();
                    let column =
                        self.column(self.d.pair_type, self.d.pair_width, pairs.into_iter(), n);
                    let out = telora_alloc(24);
                    if n > 0 {
                        heap::write(out + 8, column);
                        heap::write(out + 16, n);
                        heap::write(out + 20, ARRAY_SLICE);
                    }
                    out
                }
                _ => panic!("invalid column Map operation"),
            }
        }
    }
}
pub unsafe fn dispatch(request: u32) -> u32 {
    unsafe {
        let d = heap::read::<Request>(request);
        let mut c = Columns { d, failed: false };
        let out = c.run();
        heap::write(
            request + Request::STATUS,
            if c.failed { u32::MAX } else { 0 },
        );
        out
    }
}

/// Compiler-only normalized column map operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_column_map(request: u32) -> u32 {
    unsafe { dispatch(request) }
}
