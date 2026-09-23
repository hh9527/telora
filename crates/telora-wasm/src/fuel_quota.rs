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

    pub(crate) fn call<P: wasmi::WasmParams, R: wasmi::WasmResults>(
        &mut self,
        store: &mut wasmi::Store<wasmi::StoreLimits>,
        func: wasmi::TypedFunc<P, R>,
        params: P,
    ) -> Result<R, String> {
        let result = func.call(&mut *store, params);
        self.consumed = self
            .limit
            .saturating_sub(store.get_fuel().map_err(|e| e.to_string())?);
        result.map_err(|e| e.to_string())
    }
}
