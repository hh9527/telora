//! Thin execution host: Wasm owns language operations, values, and initialization.
use crate::{abi, artifact::Manifest};
pub(crate) mod exports;
mod timing;

// Generous execution boundaries, not an allocation accounting model. The engine
// refuses growth before allocating; its ordinary stack limits remain in force.
const MEMORY_BOUND: usize = 64 * 1024 * 1024;
static NEXT_SESSION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn session_id() -> u64 {
    NEXT_SESSION_ID
        .try_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |id| id.checked_add(1),
        )
        .expect("session identity exhausted")
}

const TABLE_BOUND: usize = 1_000_000;

pub struct Session {
    pub(crate) entry_export: &'static str,
    pub(crate) identity: u64,
    pub(crate) trapped: bool,
    pub(crate) gc_config: crate::gc::GcConfig,
    pub(crate) exports: exports::Exports,
    pub(crate) fuel_budget: u64,
    pub(crate) request_fuel: u64,
    pub(crate) initialization_quota: Option<crate::fuel_quota::FuelQuota>,
    pub(crate) execution_quota: Option<crate::fuel_quota::FuelQuota>,
    pub(crate) metered_fuel: Option<u64>,
    pub(crate) memory_limit: usize,
    pub(crate) module: wasmi::Module,
    pub usage_reporter: Option<fn(Usage)>,
    pub manifest: Manifest,
    pub(crate) store: wasmi::Store<wasmi::StoreLimits>,
    pub(crate) instance: wasmi::Instance,
    pub(crate) memory: wasmi::Memory,
    pub(crate) registered_sources: usize,
    pub(crate) emitted_debug: std::cell::Cell<u32>,
}

/// Engine boundaries and consumption, in raw fuel units and bytes (not RSS).
pub struct Usage {
    pub fuel_budget: u64,
    pub fuel_remaining: u64,
    pub memory_bytes: usize,
    pub memory_limit: usize,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(report) = self.usage_reporter {
            report(self.usage());
        }
    }
}

impl Session {
    /// A trap may interrupt publication, sweep or scratch cleanup. Only a fresh
    /// instance (or a service snapshot restore) may execute afterwards.
    pub(crate) fn ensure_live(&self) -> Result<(), String> {
        if self.trapped
            || self
                .initialization_quota
                .as_ref()
                .is_some_and(|q| q.trapped())
            || self.execution_quota.as_ref().is_some_and(|q| q.trapped())
        {
            return Err(
                "Wasm: session terminated after trap; reload or restore its snapshot".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn guest_call<P: wasmi::WasmParams, R: wasmi::WasmResults>(
        &mut self,
        func: wasmi::TypedFunc<P, R>,
        params: P,
    ) -> Result<R, String> {
        self.ensure_live()?;
        let result = func.call(&mut self.store, params);
        self.trapped |= result.is_err();
        result.map_err(|error| {
            self.allocation_error_with_growth(
                error.to_string(),
                error.as_trap_code() == Some(wasmi::TrapCode::GrowthOperationLimited),
            )
        })
    }

    pub(crate) fn allocation_error(&self, error: String) -> String {
        let growth_failure = self
            .execution_quota
            .as_ref()
            .is_some_and(|quota| quota.growth_failure())
            || self
                .initialization_quota
                .as_ref()
                .is_some_and(|quota| quota.growth_failure());
        self.allocation_error_with_growth(error, growth_failure)
    }

    pub(crate) fn allocation_error_with_growth(
        &self,
        error: String,
        growth_failure: bool,
    ) -> String {
        let memory = self.memory.data(&self.store);
        let word = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(
                memory.get(at..at.checked_add(4)?)?.try_into().ok()?,
            ))
        };
        let detail = (|| {
            let pointer = word(abi::ALLOCATION_FAILURE as usize)? as usize;
            if pointer == 0 {
                return None;
            }
            let phase = match word(pointer)? {
                1 => "dynamic",
                2 => "image",
                _ => return None,
            };
            let category = match word(pointer.checked_add(4)?)? {
                0 if growth_failure => "allocator-failure",
                0 => "interrupted",
                1 => "capacity-overflow",
                2 => "managed-budget",
                3 => "allocator-failure",
                4 => "invalid-reference",
                _ => return None,
            };
            Some(format!(
                "allocation phase={phase} category={category} requested={} limit={} allocated={} committed={}",
                word(pointer.checked_add(8)?)?,
                word(pointer.checked_add(12)?)?,
                word(pointer.checked_add(16)?)?,
                word(pointer.checked_add(20)?)?
            ))
        })();
        match detail {
            Some(detail) => format!("Wasm: {detail}: {error}"),
            None => error.to_string(),
        }
    }

    pub fn usage(&self) -> Usage {
        Usage {
            fuel_budget: self.fuel_budget,
            fuel_remaining: self
                .fuel_budget
                .saturating_sub(self.metered_fuel.unwrap_or_else(|| {
                    self.fuel_budget
                        .saturating_sub(self.store.get_fuel().unwrap_or(0))
                })),
            memory_bytes: self.memory.data_size(&self.store),
            memory_limit: self.memory_limit,
        }
    }

    pub(crate) fn fresh_instance(&self) -> Result<Self, String> {
        let debug_enabled: [u8; 4] = self
            .memory
            .data(&self.store)
            .get(abi::DEBUG_ENABLED as usize..abi::DEBUG_ENABLED as usize + 4)
            .ok_or("Wasm: missing debug protocol word")?
            .try_into()
            .unwrap();
        let limits = wasmi::StoreLimitsBuilder::new()
            .memory_size(self.memory_limit)
            .table_elements(TABLE_BOUND)
            .trap_on_grow_failure(true)
            .build();
        let mut store = wasmi::Store::new(self.module.engine(), limits);
        store.limiter(|limits| limits);
        store
            .set_fuel(self.fuel_budget)
            .map_err(|error| error.to_string())?;
        let instance = wasmi::Linker::new(self.module.engine())
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|error| error.to_string())?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("Wasm: missing memory export")?;
        memory
            .write(&mut store, abi::DEBUG_ENABLED as usize, &debug_enabled)
            .map_err(|error| error.to_string())?;
        let exports = exports::Exports::bind(instance, &store)?;
        let mut session = Self {
            entry_export: "telora_entry",
            identity: session_id(),
            trapped: false,
            gc_config: crate::gc::GcConfig::default(),
            exports,
            fuel_budget: self.fuel_budget,
            request_fuel: self.request_fuel,
            initialization_quota: None,
            execution_quota: None,
            metered_fuel: self.metered_fuel,
            memory_limit: self.memory_limit,
            module: self.module.clone(),
            // The caller transfers reporting ownership when the fresh instance
            // replaces this one; internal instance disposal is not an execution.
            usage_reporter: None,
            manifest: self.manifest.clone(),
            store,
            instance,
            memory,
            registered_sources: self.registered_sources,
            emitted_debug: std::cell::Cell::new(self.emitted_debug.get()),
        };
        session.configure_gc(self.gc_config)?;
        Ok(session)
    }
    /// Load a persistent artifact without MIR, a source loader, or a type solver.
    pub fn load(bytes: &[u8], fuel: u64) -> Result<Self, String> {
        Self::load_with_limits(bytes, fuel, MEMORY_BOUND)
    }

    pub fn load_with_limits(bytes: &[u8], fuel: u64, memory_limit: usize) -> Result<Self, String> {
        Self::load_with_limits_observed(bytes, fuel, memory_limit, &mut |phase, elapsed| {
            if std::env::var_os("TELORA_WASM_TIMINGS").is_some() {
                eprintln!(
                    "{}",
                    serde_json::json!({"wasm_phase": phase, "elapsed_ns": elapsed.as_nanos()})
                );
            }
        })
    }

    /// Load with synchronous phase observations, independent of process-wide settings.
    /// Instantiation includes the Wasm start function under the engine's public API.
    pub fn load_with_limits_observed(
        bytes: &[u8],
        fuel: u64,
        memory_limit: usize,
        observer: &mut dyn FnMut(&'static str, std::time::Duration),
    ) -> Result<Self, String> {
        let manifest = timing::observe("load_metadata", observer, || Manifest::read(bytes))?;
        let engine = timing::observe("load_engine", observer, || {
            let mut config = wasmi::Config::default();
            config.consume_fuel(true);
            // Lazy translation cannot be resumed on fuel exhaustion; meter execution only.
            config.fuel_cost(wasmi::CustomFuelCosts {
                bytes_copied_per_fuel: 64,
                fuel_per_bytes_translated: 0,
                fuel_per_bytes_validated: 0,
            });
            Ok(wasmi::Engine::new(&config))
        })?;
        let module = timing::observe("load_module", observer, || {
            wasmi::Module::new(&engine, bytes).map_err(|e| e.to_string())
        })?;
        let mut store = timing::observe("load_store", observer, || {
            let limits = wasmi::StoreLimitsBuilder::new()
                .memory_size(memory_limit)
                .table_elements(TABLE_BOUND)
                .trap_on_grow_failure(true)
                .build();
            let mut store = wasmi::Store::new(&engine, limits);
            store.limiter(|limits| limits);
            store.set_fuel(fuel).map_err(|e| e.to_string())?;
            Ok(store)
        })?;
        let instance = timing::observe("load_instance_start", observer, || {
            wasmi::Linker::new(&engine)
                .instantiate_and_start(&mut store, &module)
                .map_err(|e| e.to_string())
        })?;
        let (memory, exports) = timing::observe("load_exports", observer, || {
            let memory = instance
                .get_memory(&store, "memory")
                .ok_or("Wasm: missing memory export")?;
            let exports = exports::Exports::bind(instance, &store)?;
            Ok((memory, exports))
        })?;
        let registered_sources = manifest.sources.len();
        let mut session = Self {
            entry_export: "telora_entry",
            identity: session_id(),
            trapped: false,
            gc_config: crate::gc::GcConfig::default(),
            exports,
            module,
            fuel_budget: fuel,
            request_fuel: fuel,
            initialization_quota: None,
            execution_quota: None,
            metered_fuel: None,
            memory_limit,
            usage_reporter: None,
            manifest,
            store,
            instance,
            memory,
            registered_sources,
            emitted_debug: std::cell::Cell::new(0),
        };
        session.configure_gc(crate::gc::GcConfig {
            heap_limit: u32::try_from(memory_limit).unwrap_or(u32::MAX),
            ..crate::gc::GcConfig::default()
        })?;
        Ok(session)
    }
    pub fn initialize(&mut self) -> Result<(), String> {
        self.ensure_live()?;
        self.initialization_quota = Some(crate::fuel_quota::FuelQuota::new(self.fuel_budget));
        self.register_sources()?;
        let initialize = self
            .instance
            .get_typed_func::<(), i32>(&self.store, "telora_initialize")
            .map_err(|e| e.to_string())?;
        let status =
            self.initialization_quota
                .as_mut()
                .unwrap()
                .call(&mut self.store, initialize, ());
        let status = status.map_err(|error| self.allocation_error(error))?;
        self.metered_fuel = self
            .initialization_quota
            .as_ref()
            .map(|quota| quota.consumed());
        if status == 0 {
            return Err(self.failure());
        }
        Ok(())
    }

    pub fn set_request_fuel(&mut self, request_fuel: u64) {
        self.request_fuel = request_fuel;
    }

    pub(crate) fn start_execution(&mut self) -> Result<(), String> {
        self.ensure_live()?;
        self.fuel_budget = self.request_fuel;
        self.store
            .set_fuel(self.request_fuel)
            .map_err(|e| e.to_string())?;
        self.initialization_quota = None;
        self.execution_quota = Some(crate::fuel_quota::FuelQuota::new(self.request_fuel));
        self.metered_fuel = None;
        Ok(())
    }
    pub fn active_initialization_root(
        &self,
    ) -> Result<Option<crate::artifact::InitializationRoot>, String> {
        let index = self
            .instance
            .get_global(&self.store, "telora_initialization_root")
            .ok_or("Wasm: missing initialization root global")?
            .get(&self.store)
            .i32()
            .ok_or("Wasm: invalid initialization root global")?;
        if index == 0 {
            return Ok(None);
        }
        let index =
            usize::try_from(index - 1).map_err(|_| "Wasm: invalid initialization root identity")?;
        self.manifest
            .initialization_roots
            .get(index)
            .cloned()
            .map(Some)
            .ok_or_else(|| "Wasm: invalid initialization root identity".into())
    }
    pub(crate) fn register_sources(&mut self) -> Result<(), String> {
        while self.registered_sources < self.manifest.sources.len() {
            let source = &self.manifest.sources[self.registered_sources];
            let id = source.id;
            let name = source.name.as_bytes().to_vec();
            let lines = source
                .lines
                .iter()
                .flatten()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>();
            let pointer = self.allocate(name.len())?;
            self.write(pointer as usize, &name)?;
            let pointer = self.output().address(pointer.into(), name.len() as u64)? as u32;
            let register = self
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&self.store, "telora_register_source")
                .map_err(|e| e.to_string())?;
            if self.guest_call(register, (id as i32, pointer as i32, name.len() as i32))? == 0 {
                return Err("Wasm: source identity was registered with a different name".into());
            }
            if !lines.is_empty() {
                let pointer = self.allocate(lines.len())?;
                self.write(pointer as usize, &lines)?;
                let pointer = self.output().address(pointer.into(), lines.len() as u64)? as u32;
                let index = self
                    .instance
                    .get_typed_func::<(u32, u32, u32), ()>(&self.store, "telora_source_index")
                    .map_err(|e| e.to_string())?;
                self.guest_call(index, (id, pointer, (lines.len() / 8) as u32))?;
            }
            self.manifest.sources[self.registered_sources].lines.clear();
            self.registered_sources += 1;
        }
        Ok(())
    }
    pub fn eval(&mut self) -> Result<serde_json::Value, String> {
        self.start_execution()?;
        let pointer = self.entry()?;
        self.json(pointer)
    }
    pub(crate) fn entry(&mut self) -> Result<u32, String> {
        if self.execution_quota.is_none() {
            self.start_execution()?;
        }
        let entry = self
            .instance
            .get_typed_func::<(), i32>(&self.store, self.entry_export)
            .map_err(|e| e.to_string())?;
        let result = self
            .execution_quota
            .as_mut()
            .unwrap()
            .call(&mut self.store, entry, ());
        self.metered_fuel = self.execution_quota.as_ref().map(|quota| quota.consumed());
        let pointer = result.map_err(|error| self.allocation_error(error))? as u32;
        if pointer == abi::NULL {
            return Err(self.failure());
        }
        Ok(pointer)
    }
    /// Direct typed invocation for artifact consumers; service calls use their
    /// compiler-owned sealed init/transform contract.
    pub fn call(&mut self, arguments: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        self.start_execution()?;
        let pointer = self.entry()?;
        let descriptor = &self.manifest.types[self.manifest.entry_type as usize];
        if descriptor.kind != crate::artifact::Kind::Function
            || descriptor.arguments.len() != arguments.len() + 1
        {
            return Err("Wasm: entry call does not match its sealed signature".into());
        }
        let signature = descriptor.arguments.clone();
        let args = self.allocate(
            arguments
                .len()
                .checked_add(1)
                .ok_or("Wasm: argument count overflow")?
                .checked_mul(4)
                .ok_or("Wasm: argument size overflow")?,
        )?;
        for (index, (value, &ty)) in arguments.iter().zip(&signature).enumerate() {
            let value = self.input(ty, value, 0)?;
            self.write(args as usize + index * 4, &value.to_le_bytes())?;
        }
        // Host invocation has no Telora computation expression to blame.
        self.write(args as usize + arguments.len() * 4, &0u32.to_le_bytes())?;
        let output =
            self.allocate(self.manifest.types[*signature.last().unwrap() as usize].bytes as usize)?;
        let invoke = self
            .instance
            .get_typed_func::<(i32, i32, i32), i32>(&self.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        let called = self.execution_quota.as_mut().unwrap().call(
            &mut self.store,
            invoke,
            (pointer as i32, args as i32, output as i32),
        );
        self.metered_fuel = self.execution_quota.as_ref().map(|quota| quota.consumed());
        let result = called.map_err(|error| self.allocation_error(error))? as u32;
        if result == abi::NULL {
            return Err(self.failure());
        }
        crate::output::Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        }
        .json(result as u64, *signature.last().unwrap(), 0)
    }
    fn json(&self, pointer: u32) -> Result<serde_json::Value, String> {
        crate::output::Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        }
        .json(pointer as u64, self.manifest.entry_type, 0)
    }
    pub(crate) fn failure(&self) -> String {
        match self.active_failure() {
            Ok(Some(diagnostic)) => return diagnostic.render(&self.manifest),
            Err(message) => return message,
            Ok(None) => {}
        }
        match self.diagnostics() {
            Ok(diagnostics) => diagnostics
                .iter()
                .rev()
                .find(|d| d.severity == telora_core::source::Severity::Error)
                .map(|d| d.render(&self.manifest))
                .unwrap_or_else(|| "Wasm session is not initialized or has failed".into()),
            Err(message) => message,
        }
    }
}
