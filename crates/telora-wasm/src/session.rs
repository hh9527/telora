//! Thin execution host: Wasm owns language operations, values, and initialization.
use crate::{
    abi,
    artifact::{Kind, Manifest},
};

pub struct Session {
    pub manifest: Manifest,
    store: wasmi::Store<()>,
    instance: wasmi::Instance,
    memory: wasmi::Memory,
}

impl Session {
    /// Load a persistent artifact without MIR, a source loader, or a type solver.
    pub fn load(bytes: &[u8], fuel: u64) -> Result<Self, String> {
        let manifest = Manifest::read(bytes)?;
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, bytes).map_err(|e| e.to_string())?;
        let mut store = wasmi::Store::new(&engine, ());
        store.set_fuel(fuel).map_err(|e| e.to_string())?;
        let instance = wasmi::Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| e.to_string())?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("Wasm: missing memory export")?;
        Ok(Self {
            manifest,
            store,
            instance,
            memory,
        })
    }
    pub fn initialize(&mut self) -> Result<(), String> {
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
    pub fn eval(&mut self) -> Result<serde_json::Value, String> {
        let entry = self
            .instance
            .get_typed_func::<(), i32>(&self.store, "telora_entry")
            .map_err(|e| e.to_string())?;
        let pointer = entry.call(&mut self.store, ()).map_err(|e| e.to_string())? as u32;
        if pointer == abi::NULL {
            return Err(self.failure());
        }
        self.json(pointer)
    }
    fn json(&self, pointer: u32) -> Result<serde_json::Value, String> {
        let memory = self.memory.data(&self.store);
        let get = |offset: usize, length: usize| {
            memory
                .get(pointer as usize + offset..pointer as usize + offset + length)
                .ok_or("Wasm: invalid result address")
        };
        let ty = u32::from_le_bytes(get(12, 4)?.try_into().unwrap());
        let kind = self
            .manifest
            .types
            .get(ty as usize)
            .ok_or("Wasm: invalid result TypeId")?
            .kind;
        if kind == Kind::Unit {
            return Ok(serde_json::Value::Null);
        }
        let raw = i64::from_le_bytes(get(16, 8)?.try_into().unwrap());
        Ok(match kind {
            Kind::Int => raw.into(),
            Kind::Float => serde_json::Number::from_f64(f64::from_bits(raw as u64))
                .ok_or("Wasm: non-finite Float cannot be serialized")?
                .into(),
            Kind::Bool => (raw != 0).into(),
            _ => return Err("Wasm: result JSON encoding is not implemented for this type".into()),
        })
    }
    fn failure(&self) -> String {
        let Some(global) = self.instance.get_global(&self.store, "telora_error") else {
            return "Wasm execution failed".into();
        };
        let pointer = global.get(&self.store).i32().unwrap_or(0) as u32 as usize;
        if pointer == 0 {
            return "Wasm session is not initialized or has failed".into();
        }
        let Some(bytes) = self.memory.data(&self.store).get(pointer..pointer + 16) else {
            return "Wasm invalid error address".into();
        };
        let words = bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let message = match words[3] {
            abi::ERROR_OVERFLOW => "integer arithmetic overflowed",
            abi::ERROR_DIVISION => "integer division by zero",
            abi::ERROR_CYCLE => "initialization dependency cycle",
            _ => "Wasm execution failed",
        };
        let source = self
            .manifest
            .sources
            .iter()
            .find(|s| s.id == words[0])
            .map(|s| s.name.as_str())
            .unwrap_or("<unknown>");
        match self
            .manifest
            .locations
            .iter()
            .find(|l| l.source == words[0] && l.start == words[1] && l.end == words[2])
        {
            Some(loc) => format!("{source}:{}:{}: {message}", loc.line, loc.column),
            None => format!("{source}:{}..{}: {message}", words[1], words[2]),
        }
    }
}
