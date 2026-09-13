//! Thin execution host: Wasm owns language operations, values, and initialization.
use crate::{abi, artifact::Manifest};

pub struct Session {
    pub manifest: Manifest,
    pub(crate) store: wasmi::Store<()>,
    pub(crate) instance: wasmi::Instance,
    pub(crate) memory: wasmi::Memory,
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
    /// Direct typed invocation used by the independent artifact host. The CLI's
    /// eval-with adapter will supply its already sealed entry contract separately.
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
            abi::ERROR_INDEX => "array index out of bounds",
            abi::ERROR_KEY => "dictionary key is absent",
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
