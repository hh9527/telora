pub(crate) struct FuelQuota {
    limit: u64,
    consumed: u64,
    trapped: bool,
    growth_failure: bool,
}

impl FuelQuota {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            limit,
            consumed: 0,
            trapped: false,
            growth_failure: false,
        }
    }

    pub(crate) fn trapped(&self) -> bool {
        self.trapped
    }

    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    pub(crate) fn growth_failure(&self) -> bool {
        self.growth_failure
    }

    pub(crate) fn call<P: wasmi::WasmParams, R: wasmi::WasmResults>(
        &mut self,
        store: &mut wasmi::Store<wasmi::StoreLimits>,
        func: wasmi::TypedFunc<P, R>,
        params: P,
    ) -> Result<R, String> {
        if self.trapped {
            return Err("Wasm: session terminated after trap".into());
        }
        let result = func.call(&mut *store, params);
        self.trapped |= result.is_err();
        self.growth_failure = result.as_ref().err().and_then(|error| error.as_trap_code())
            == Some(wasmi::TrapCode::GrowthOperationLimited);
        self.consumed = self
            .limit
            .saturating_sub(store.get_fuel().map_err(|e| e.to_string())?);
        result.map_err(|e| e.to_string())
    }
}
