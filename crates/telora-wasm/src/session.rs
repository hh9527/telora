//! Thin execution host: Wasm owns language operations, values, and initialization.
use crate::{abi, artifact::Manifest};

// Generous execution boundaries, not an allocation accounting model. The engine
// refuses growth before allocating; its ordinary stack limits remain in force.
const MEMORY_BOUND: usize = 1024 * 1024 * 1024;
const TABLE_BOUND: usize = 1_000_000;

pub struct Session {
    pub(crate) fuel_budget: u64,
    pub(crate) memory_limit: usize,
    pub(crate) module: wasmi::Module,
    pub usage_reporter: Option<fn(Usage)>,
    pub manifest: Manifest,
    pub(crate) store: wasmi::Store<wasmi::StoreLimits>,
    pub(crate) instance: wasmi::Instance,
    pub(crate) memory: wasmi::Memory,
    pub(crate) registered_sources: usize,
    pub(crate) emitted_debug: std::cell::Cell<u32>,
    pub(crate) trace_types: u32,
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
    pub fn usage(&self) -> Usage {
        Usage {
            fuel_budget: self.fuel_budget,
            fuel_remaining: self.store.get_fuel().unwrap_or(0),
            memory_bytes: self.memory.data_size(&self.store),
            memory_limit: self.memory_limit,
        }
    }
    /// Load a persistent artifact without MIR, a source loader, or a type solver.
    pub fn load(bytes: &[u8], fuel: u64) -> Result<Self, String> {
        Self::load_with_limits(bytes, fuel, MEMORY_BOUND)
    }

    pub fn load_with_limits(bytes: &[u8], fuel: u64, memory_limit: usize) -> Result<Self, String> {
        let manifest = Manifest::read(bytes)?;
        let bundled_data = crate::bundle::read(bytes, &manifest)?;
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, bytes).map_err(|e| e.to_string())?;
        let limits = wasmi::StoreLimitsBuilder::new()
            .memory_size(memory_limit)
            .table_elements(TABLE_BOUND)
            .trap_on_grow_failure(true)
            .build();
        let mut store = wasmi::Store::new(&engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(fuel).map_err(|e| e.to_string())?;
        let instance = wasmi::Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| e.to_string())?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("Wasm: missing memory export")?;
        let mut session = Self {
            module,
            fuel_budget: fuel,
            memory_limit,
            usage_reporter: None,
            manifest,
            store,
            instance,
            memory,
            registered_sources: 0,
            emitted_debug: std::cell::Cell::new(0),
            trace_types: 0,
        };
        for module in bundled_data {
            session.inject_data_packet(module.symbol, &module.packet)?;
        }
        Ok(session)
    }
    pub fn initialize(&mut self) -> Result<(), String> {
        self.prepare_collection()?;
        self.register_sources()?;
        let initialize = self
            .instance
            .get_typed_func::<(), i32>(&self.store, "telora_initialize")
            .map_err(|e| e.to_string())?;
        let status = initialize
            .call(&mut self.store, ())
            .map_err(|e| e.to_string())?;
        if status == 0 {
            return Err(self.failure());
        }
        Ok(())
    }
    pub(crate) fn register_sources(&mut self) -> Result<(), String> {
        while self.registered_sources < self.manifest.sources.len() {
            let source = &self.manifest.sources[self.registered_sources];
            let id = source.id;
            let name = source.name.as_bytes().to_vec();
            let pointer = self.allocate(name.len())?;
            self.write(pointer as usize, &name)?;
            let register = self
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&self.store, "telora_register_source")
                .map_err(|e| e.to_string())?;
            if register
                .call(
                    &mut self.store,
                    (id as i32, pointer as i32, name.len() as i32),
                )
                .map_err(|e| e.to_string())?
                == 0
            {
                return Err("Wasm: source identity was registered with a different name".into());
            }
            self.registered_sources += 1;
        }
        Ok(())
    }
    pub fn eval(&mut self) -> Result<serde_json::Value, String> {
        let pointer = self.entry()?;
        self.json(pointer)
    }
    pub(crate) fn entry(&mut self) -> Result<u32, String> {
        let entry = self
            .instance
            .get_typed_func::<(), i32>(&self.store, "telora_entry")
            .map_err(|e| e.to_string())?;
        let pointer = entry.call(&mut self.store, ()).map_err(|e| e.to_string())? as u32;
        if pointer == abi::NULL {
            return Err(self.failure());
        }
        Ok(pointer)
    }
    /// Direct typed invocation for artifact consumers; service calls use their
    /// compiler-owned sealed init/transform contract.
    pub fn call(&mut self, arguments: &[serde_json::Value]) -> Result<serde_json::Value, String> {
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
                .checked_mul(4)
                .ok_or("Wasm: argument size overflow")?,
        )?;
        for (index, (value, &ty)) in arguments.iter().zip(&signature).enumerate() {
            let value = self.input(ty, value, 0)?;
            self.write(args as usize + index * 4, &value.to_le_bytes())?;
        }
        let invoke = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        let result = invoke
            .call(&mut self.store, (pointer as i32, args as i32))
            .map_err(|e| e.to_string())? as u32;
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
        match self.diagnostics() {
            Ok(diagnostics) => diagnostics
                .iter()
                .rev()
                .find(|d| !d.warning)
                .map(|d| d.render(&self.manifest))
                .unwrap_or_else(|| "Wasm session is not initialized or has failed".into()),
            Err(message) => message,
        }
    }
}
