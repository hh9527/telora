use crate::backend::runtime::{self, Store, StoreLimits, TypedFunc};
use anyhow::Result;

pub(crate) struct FuelQuota {
    limit: u64,
    consumed: u64,
}

impl FuelQuota {
    pub(crate) fn new(limit: u64) -> Self {
        Self { limit, consumed: 0 }
    }

    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    pub(crate) fn call<P: runtime::WasmParams, R: runtime::WasmResults>(
        &mut self,
        store: &mut Store<StoreLimits>,
        func: TypedFunc<P, R>,
        params: P,
    ) -> Result<R> {
        let result = func.call(&mut *store, params);
        self.consumed = self.limit.saturating_sub(store.get_fuel()?);
        Ok(result?)
    }
}
