//! A temporary publication destination. The running graph remains owned and is
//! restored unchanged after export; only the image uses continuous coordinates.
use crate::{abi::*, tables::Table};
use alloc::vec::Vec;

pub(crate) struct Packing {
    tables: [Table; TABLE_COUNT as usize],
    demands: Vec<u8>,
    handler: Option<u32>,
    content: u32,
    stats: [u32; 5],
}
impl Packing {
    pub(crate) unsafe fn begin(service: bool) -> Self {
        unsafe {
            let saved = Self {
                tables: crate::tables::snapshot(),
                demands: crate::collect::snapshot_demands(),
                handler: service.then(|| crate::service::snapshot()),
                content: crate::content::len(),
                stats: crate::collect::snapshot_stats(),
            };
            crate::heap::start_packing();
            if service {
                crate::collect::collect_initialization();
            } else {
                crate::collect::collect_data();
            }
            saved
        }
    }
}
impl Drop for Packing {
    fn drop(&mut self) {
        unsafe {
            crate::heap::finish_packing();
            crate::tables::restore(self.tables);
            crate::collect::restore_demands(&self.demands);
            crate::collect::restore_stats(self.stats);
            crate::content::restore(self.content);
            if let Some(handler) = self.handler {
                crate::service::replace_handler(handler);
            }
        }
    }
}
