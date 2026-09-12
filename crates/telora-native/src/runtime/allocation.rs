use super::*;
use std::cell::Cell;

pub(super) struct Allocation {
    limit: u64,
    requested: Cell<u64>,
    exhausted: Cell<bool>,
}
impl Default for Allocation {
    fn default() -> Self { Self { limit: u64::MAX, requested: Cell::new(0), exhausted: Cell::new(false) } }
}
impl Runtime {
    pub(crate) fn charge_tail_words(&self, words: usize) -> Result<()> {
        self.charge_allocation(words, 8, 0)
    }
    pub(super) fn charge_blame(&self, message_words: usize, subjects: usize) -> Result<()> {
        self.charge_allocation(message_words, 8, std::mem::size_of::<blame::Blame>())?;
        self.charge_allocation(subjects, std::mem::size_of::<crate::abi::Origin>(), 0)
    }
    pub(super) fn charge_test(&self, lengths: impl ExactSizeIterator<Item = usize>) -> Result<()> {
        self.charge_allocation(lengths.len(), std::mem::size_of::<Box<[u64]>>(), std::mem::size_of::<test_description::TestDescription>())?;
        for length in lengths { self.charge_allocation(length, 8, 0)?; }
        Ok(())
    }
    pub fn with_allocation_limit(mut self, bytes: u64) -> Self { self.allocation.limit = bytes; self }
    pub fn requested_allocation_bytes(&self) -> u64 { self.allocation.requested.get() }
    pub fn allocation_exhausted(&self) -> bool { self.allocation.exhausted.get() }
    pub(super) fn remaining_allocation_bytes(&self) -> u64 {
        self.allocation.limit.saturating_sub(self.allocation.requested.get())
    }
    pub(super) fn charge_allocation(&self, count: usize, width: usize, overhead: usize) -> Result<()> {
        if self.allocation_exhausted() { return Err("native allocation byte limit exceeded".into()); }
        let total = (count as u64).checked_mul(width as u64).and_then(|bytes| bytes.checked_add(overhead as u64))
            .and_then(|bytes| self.allocation.requested.get().checked_add(bytes));
        let Some(total) = total else {
            self.allocation.exhausted.set(true);
            return Err("native allocation byte count overflow".into());
        };
        self.allocation.requested.set(total);
        if total > self.allocation.limit {
            self.allocation.exhausted.set(true);
            return Err("native allocation byte limit exceeded".into());
        }
        Ok(())
    }
}
