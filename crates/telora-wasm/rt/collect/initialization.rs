//! Conservative initialization roots. Definitions are not values until Ready.
use super::{Collector, DEMANDS};
use crate::{abi::*, values::word};

static mut STATS: [u32; 5] = [0; 5];

/// Arena bytes before/after, demand roots, and linear memory before/after GC.
/// Wasm cannot shrink its memory, so the latter includes the collection peak.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_initialization_stat(index: u32) -> u32 {
    unsafe { (&*core::ptr::addr_of!(STATS))[index as usize] }
}

pub(crate) unsafe fn collect() {
    unsafe {
        // Source names and BOLs have independent ownership and remain available
        // even when no retained language value currently mentions that source.
        crate::sources::freeze();
        let before = crate::heap::telora_heap_bytes();
        let memory_before = crate::telora_heap_end();
        let mut gc = Collector::begin(true);
        let (demands, count) = DEMANDS;
        let mut roots = 0;
        for index in 0..count {
            let slot = demands + index * DEMAND_BYTES;
            if word(slot, 0) == 2 {
                roots += 1;
                let value = gc.value(word(slot, 4));
                gc.put(slot + 4, value);
            }
        }
        crate::service::collect_initialization(&mut gc);
        // Initialization callers still need to read these events. Preserving
        // their order also preserves Host diagnostic/debug cursors.
        for table in [DIAGNOSTICS, DEBUG_EVENTS] {
            for id in 0..gc.old[table as usize].length {
                assert_eq!(gc.object(table, id), id);
            }
        }
        gc.finish();
        STATS = [before, crate::heap::telora_heap_bytes(), roots,
            memory_before, crate::telora_heap_end()];
    }
}
