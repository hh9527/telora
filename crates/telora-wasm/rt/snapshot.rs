//! Portable initialized state for moving a compact service into a fresh instance.
use crate::{abi::*, tables::Table};
use alloc::{string::String, vec::Vec};

const MAGIC: [u8; 8] = *b"TELSNAP1";

struct Writer(Vec<u8>);

impl Writer {
    fn word(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.word(u32::try_from(value.len()).expect("snapshot field exceeds wasm32"));
        self.0.extend_from_slice(value);
    }
    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> &'a [u8] {
        let end = self.at.checked_add(len).expect("snapshot offset overflow");
        let result = self.bytes.get(self.at..end).expect("truncated snapshot");
        self.at = end;
        result
    }
    fn word(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn bytes(&mut self) -> &'a [u8] {
        let len = self.word() as usize;
        self.take(len)
    }
    fn text(&mut self) -> String {
        String::from(core::str::from_utf8(self.bytes()).expect("invalid snapshot text"))
    }
    fn finish(self) {
        assert_eq!(self.at, self.bytes.len(), "trailing snapshot bytes");
    }
}

unsafe fn encode() -> Vec<u8> {
    unsafe {
        let mut out = Writer(Vec::new());
        out.0.extend_from_slice(&MAGIC);
        out.word(VERSION);
        let (origin, words) = crate::heap::snapshot();
        out.word(origin);
        out.bytes(&words);
        out.bytes(&crate::content::snapshot());

        let tables = crate::tables::snapshot();
        out.bytes(core::slice::from_raw_parts(
            tables.as_ptr().cast::<u8>(),
            core::mem::size_of_val(&tables),
        ));
        out.bytes(&crate::collect::snapshot_demands());
        for stat in crate::collect::snapshot_stats() {
            out.word(stat);
        }
        out.word(crate::service::snapshot());

        let sources = crate::sources::snapshot();
        out.word(sources.len().try_into().unwrap());
        for source in sources {
            out.word(source.id);
            out.bytes(&source.name);
            out.word(source.lines.len().try_into().unwrap());
            for line in source.lines {
                out.word(line);
            }
        }

        let service_sources = crate::service_sources::snapshot();
        out.word(service_sources.len().try_into().unwrap());
        for source in service_sources {
            out.word(source.id);
            out.bytes(&source.name);
        }

        let regexes = crate::regex::snapshot();
        out.word(regexes.len().try_into().unwrap());
        for pattern in regexes {
            out.text(&pattern);
        }
        out.0
    }
}

unsafe fn decode(bytes: &[u8]) {
    unsafe {
        let mut input = Reader { bytes, at: 0 };
        assert_eq!(input.take(MAGIC.len()), MAGIC);
        assert_eq!(input.word(), VERSION, "snapshot ABI mismatch");
        let origin = input.word();
        crate::heap::restore(origin, input.bytes());
        crate::content::restore(input.bytes());

        let raw_tables = input.bytes();
        assert_eq!(
            raw_tables.len(),
            core::mem::size_of::<[Table; TABLE_COUNT as usize]>()
        );
        let tables = raw_tables
            .as_ptr()
            .cast::<[Table; TABLE_COUNT as usize]>()
            .read_unaligned();
        crate::tables::restore(tables);
        crate::collect::restore_demands(input.bytes());
        crate::collect::restore_stats(core::array::from_fn(|_| input.word()));
        let handler = input.word();

        let source_count = input.word();
        let mut sources = Vec::with_capacity(source_count as usize);
        for _ in 0..source_count {
            let id = input.word();
            let name = input.bytes().to_vec();
            let line_count = input.word();
            let lines = (0..line_count).map(|_| input.word()).collect();
            sources.push(crate::sources::Snapshot { id, name, lines });
        }
        crate::sources::restore(sources);

        let service_source_count = input.word();
        let mut service_sources = Vec::with_capacity(service_source_count as usize);
        for _ in 0..service_source_count {
            service_sources.push(crate::service_sources::Snapshot {
                id: input.word(),
                name: input.bytes().to_vec(),
            });
        }
        crate::service_sources::restore(service_sources);

        let regex_count = input.word();
        let regexes = (0..regex_count).map(|_| input.text()).collect::<Vec<_>>();
        input.finish();
        crate::regex::restore(&regexes);
        crate::service::restore(handler);
    }
}

/// Result points to two u32 words: owned pointer and exact capacity/length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_snapshot_export(result: u32) {
    unsafe {
        crate::host_memory::range(result, 8, 4);
        let bytes = encode();
        let len = u32::try_from(bytes.len()).expect("snapshot exceeds wasm32");
        let pointer = crate::host_memory::alloc(len, 1);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
        (result as *mut u32).write(pointer);
        (result as *mut u32).add(1).write(len);
    }
}

/// Input remains Host-owned and is borrowed only for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_snapshot_import(pointer: u32, length: u32) {
    unsafe {
        crate::host_memory::range(pointer, length, 1);
        decode(core::slice::from_raw_parts(pointer as *const u8, length as usize));
    }
}
