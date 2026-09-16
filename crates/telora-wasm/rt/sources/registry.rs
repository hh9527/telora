//! Source metadata owns ordinary Rust allocations, independently of language GC.
use alloc::{boxed::Box, collections::BTreeSet, vec::Vec};
use super::Source;

struct Owned {
    source: Source,
    _name: Box<[u8]>,
    lines: Option<Box<[u32]>>,
}

static mut OWNED: Vec<Owned> = Vec::new();
static mut VIEW: Vec<Source> = Vec::new();
static mut FROZEN: usize = 0;

pub(super) unsafe fn records() -> &'static [Source] {
    unsafe { &*core::ptr::addr_of!(VIEW) }
}

unsafe fn publish() {
    unsafe {
        let owned = &*core::ptr::addr_of!(OWNED);
        let view = &mut *core::ptr::addr_of_mut!(VIEW);
        view.clear();
        view.extend(owned.iter().map(|item| item.source));
        let descriptor = crate::abi::SOURCE_REGISTRY as *mut u32;
        descriptor.write(view.as_ptr() as u32);
        descriptor.add(1).write(u32::try_from(view.len()).unwrap());
    }
}

pub(super) unsafe fn register(id: u32, name: &[u8]) -> u32 {
    unsafe {
        let owned = &mut *core::ptr::addr_of_mut!(OWNED);
        if let Some(old) = owned.iter().find(|item| item.source.id == id) {
            return u32::from(old._name.as_ref() == name);
        }
        let name = name.to_vec().into_boxed_slice();
        let source = Source { id, pointer: name.as_ptr() as u32,
            length: u32::try_from(name.len()).unwrap(), lines: 0, line_count: 0 };
        owned.push(Owned { source, _name: name, lines: None });
        publish();
        1
    }
}

pub(super) unsafe fn index(id: u32, pointer: u32, count: u32, static_storage: bool) {
    unsafe {
        let owned = &mut *core::ptr::addr_of_mut!(OWNED);
        let item = owned.iter_mut().find(|item| item.source.id == id)
            .expect("unregistered source index");
        let bytes = count.checked_mul(8).unwrap() as usize;
        if item.source.line_count != 0 {
            assert_eq!(item.source.line_count, count, "source index changed");
            assert_eq!(core::slice::from_raw_parts(item.source.lines as *const u8, bytes),
                core::slice::from_raw_parts(pointer as *const u8, bytes), "source index changed");
            return;
        }
        item.source.lines = if static_storage { pointer } else {
            let words = (0..count.checked_mul(2).unwrap())
                .map(|index| ((pointer + index * 4) as *const u32).read_unaligned())
                .collect::<Vec<_>>().into_boxed_slice();
            let address = words.as_ptr() as u32;
            item.lines = Some(words);
            address
        };
        item.source.line_count = count;
        publish();
    }
}

pub(super) unsafe fn freeze() {
    unsafe { FROZEN = (&*core::ptr::addr_of!(OWNED)).len(); }
}

pub(super) unsafe fn collect(live: &BTreeSet<u32>) {
    unsafe {
        let mut index = 0;
        (&mut *core::ptr::addr_of_mut!(OWNED)).retain(|item| {
            let keep = index < FROZEN || live.contains(&item.source.id);
            index += 1;
            keep
        });
        publish();
    }
}
