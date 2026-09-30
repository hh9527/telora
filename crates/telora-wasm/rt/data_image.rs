//! Compiled data-module values, before any language or service initialization.
use crate::{abi::*, tables::Table};
use alloc::vec::Vec;

const MAGIC: &[u8; 8] = b"TELDATA1";

fn word(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn bytes(out: &mut Vec<u8>, value: &[u8]) {
    word(
        out,
        value.len().try_into().expect("data image exceeds wasm32"),
    );
    out.extend_from_slice(value);
}

struct Reader<'a> {
    input: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> &'a [u8] {
        let end = self
            .at
            .checked_add(length)
            .expect("data image offset overflow");
        let value = self.input.get(self.at..end).expect("truncated data image");
        self.at = end;
        value
    }
    fn word(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn bytes(&mut self) -> &'a [u8] {
        let length = self.word() as usize;
        self.take(length)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_data_image_export(result: u32) {
    unsafe {
        crate::host_memory::range(result, 8, 4);
        // Ready data demands are roots; parse plans and copied source spans are not.
        crate::collect::collect_data();
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        word(&mut out, VERSION);
        let (origin, words) = crate::heap::snapshot();
        word(&mut out, origin);
        bytes(&mut out, &words);
        bytes(&mut out, &crate::content::snapshot());
        let tables = crate::tables::snapshot();
        bytes(
            &mut out,
            core::slice::from_raw_parts(tables.as_ptr().cast(), core::mem::size_of_val(&tables)),
        );
        bytes(&mut out, &crate::collect::snapshot_demands());
        let sources = crate::sources::snapshot();
        word(&mut out, sources.len().try_into().unwrap());
        for source in sources {
            word(&mut out, source.id);
            bytes(&mut out, &source.name);
            word(&mut out, source.lines.len().try_into().unwrap());
            for line in source.lines {
                word(&mut out, line);
            }
        }
        let length = out.len().try_into().unwrap();
        let pointer = crate::host_memory::alloc(length, 1);
        core::ptr::copy_nonoverlapping(out.as_ptr(), pointer as *mut u8, out.len());
        (result as *mut u32).write(pointer);
        (result as *mut u32).add(1).write(length);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_data_image_import(pointer: u32, length: u32) {
    unsafe {
        crate::host_memory::range(pointer, length, 1);
        let mut input = Reader {
            input: core::slice::from_raw_parts(pointer as *const u8, length as usize),
            at: 0,
        };
        assert_eq!(input.take(MAGIC.len()), MAGIC);
        assert_eq!(input.word(), VERSION, "data image ABI mismatch");
        let origin = input.word();
        crate::heap::restore_data(origin, input.bytes());
        crate::content::restore_data(input.bytes());
        let raw = input.bytes();
        assert_eq!(
            raw.len(),
            core::mem::size_of::<[Table; TABLE_COUNT as usize]>()
        );
        let tables = raw
            .as_ptr()
            .cast::<[Table; TABLE_COUNT as usize]>()
            .read_unaligned();
        for (index, table) in tables.into_iter().enumerate() {
            assert_eq!(table.frozen, 0, "data image is already initialized");
            (table_address(index as u32) as *mut Table).write(table);
        }
        crate::collect::restore_demands(input.bytes());
        let count = input.word();
        assert!(count as usize <= (input.input.len() - input.at) / 12);
        let mut sources = Vec::new();
        for _ in 0..count {
            let id = input.word();
            let name = input.bytes().to_vec();
            let count = input.word();
            assert!(count as usize <= (input.input.len() - input.at) / 4);
            let lines = (0..count).map(|_| input.word()).collect();
            sources.push(crate::sources::Snapshot { id, name, lines });
        }
        assert_eq!(input.at, input.input.len(), "trailing data image bytes");
        crate::sources::restore(sources);
    }
}
