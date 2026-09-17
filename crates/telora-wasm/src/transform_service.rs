//! Byte-only service host. Guest owns Context, initialization and the handler.
use crate::{artifact::Manifest, session::Session};
use telora_core::data_plan::Format;
mod buffers;
mod input;

struct Baseline {
    memory: Vec<u8>,
    globals: Vec<(String, wasmi::Val)>,
    manifest: Manifest,
    registered_sources: usize,
    emitted_debug: u32,
}

pub struct SourceInput<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    pub format: Format,
}

/// Opened lazily by the Host; bytes are read directly into Guest memory.
pub struct SourceReader<'a> {
    pub name: String,
    pub reader: Box<dyn std::io::Read + 'a>,
    pub format: Format,
}

pub struct Initialization {
    pub success: bool,
    pub diagnostics: serde_json::Value,
}

pub struct TransformSession {
    session: Session,
    baseline: Option<Baseline>,
    sources: Vec<String>,
    source_ids: Vec<u32>,
    ready: bool,
    poisoned: bool,
}

impl TransformSession {
    #[cfg(feature = "snapshot")]
    pub fn snapshot(&mut self, context: &crate::snapshot::ModuleContext<'_>) -> Result<Vec<u8>, String> {
        if !self.ready || self.poisoned { return Err("snapshot requires successful initialization".into()); }
        crate::snapshot::capture(context, &mut self.session)
    }

    pub fn new(mut session: Session) -> Result<Self, String> {
        let count = session.exports.source_count
            .call(&mut session.store, ())
            .map_err(|e| e.to_string())?;
        let result = buffers::alloc(&mut session, 12, 4)?;
        let get = session.exports.source_name;
        let mut sources = Vec::new();
        let mut source_ids = Vec::new();
        for index in 0..count {
            get.call(&mut session.store, (index, result))
                .map_err(|e| e.to_string())?;
            let [id, pointer, length] = buffers::words(&session, result)?;
            sources.push(
                String::from_utf8(buffers::bytes(&session, pointer, length)?)
                    .map_err(|e| e.to_string())?,
            );
            source_ids.push(id);
        }
        buffers::free(&mut session, result, 12, 4)?;
        Ok(Self {
            session,
            baseline: None,
            sources,
            source_ids,
            ready: false,
            poisoned: false,
        })
    }

    pub fn sources(&self) -> &[String] {
        &self.sources
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn usage(&self) -> crate::session::Usage {
        let mut usage = self.session.usage();
        if let Some(baseline) = &self.baseline {
            usage.memory_limit = usage.memory_limit.saturating_add(baseline.memory.len());
        }
        usage
    }

    pub fn initialize(&mut self, sources: &[SourceInput<'_>]) -> Result<Initialization, String> {
        self.initialize_readers(sources.iter().map(|source| Ok(SourceReader {
            name: source.name.to_owned(),
            reader: Box::new(std::io::Cursor::new(source.data)),
            format: source.format,
        })), u32::MAX as usize)
    }

    pub fn initialize_readers<'a>(
        &mut self,
        sources: impl IntoIterator<Item = Result<SourceReader<'a>, String>>,
        max_bytes: usize,
    ) -> Result<Initialization, String> {
        if self.poisoned || self.ready {
            return Err("service initialization cannot be repeated".into());
        }
        self.poisoned = true;
        let mut buffer = input::TransferBuffer::new();
        let set = self.session.exports.set_source;
        let mut supplied = 0;
        for source in sources {
            let mut source = source?;
            if self.sources.get(supplied) != Some(&source.name) {
                return Err("service sources do not match declared sources".into());
            }
            let id = self.source_ids[supplied];
            let length = buffer.read(&mut self.session, &mut source.reader, max_bytes)
                .map_err(|error| format!("service source {:?}: {error}", source.name))?;
            let format = match source.format {
                Format::Json => 1,
                Format::Yaml => 2,
                Format::Toml => 3,
            };
            set.call(
                &mut self.session.store,
                (id, buffer.pointer, length, format),
            )
            .map_err(|e| e.to_string())?;
            supplied += 1;
        }
        buffer.free(&mut self.session)?;
        if supplied != self.sources.len() {
            return Err("service sources do not match declared sources".into());
        }
        let status = self.session.exports.create_service
            .call(&mut self.session.store, ())
            .map_err(|e| e.to_string())?;
        let diagnostics = self.initialization_diagnostics()?;
        self.ready = status == 0;
        // A failed initialization has no baseline and can never be retried.
        self.poisoned = !self.ready;
        Ok(Initialization {
            success: self.ready,
            diagnostics,
        })
    }

    fn initialization_diagnostics(&mut self) -> Result<serde_json::Value, String> {
        let result = buffers::alloc(&mut self.session, 12, 4)?;
        self.session.exports.diagnostics
            .call(&mut self.session.store, (1, 0, result))
            .map_err(|e| e.to_string())?;
        let bytes = buffers::response(&mut self.session, result)?;
        let diagnostics: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid initialization diagnostics: {e}"))?;
        if !diagnostics.is_array() {
            return Err("invalid initialization diagnostic response".into());
        }
        Ok(diagnostics)
    }

    /// All Host-owned ABI buffers have been freed before snapshot or reset.
    pub fn seal_initialization(&mut self) -> Result<(), String> {
        if self.baseline.is_some() {
            return Err("service initialization is already sealed".into());
        }
        if !self.ready || self.poisoned {
            return Err("service is not initialized".into());
        }
        let globals = self
            .session
            .instance
            .exports(&self.session.store)
            .filter_map(|export| {
                let name = export.name().to_owned();
                if !name.starts_with("telora_reset_global_") {
                    return None;
                }
                let global = export.into_global()?;
                global
                    .ty(&self.session.store)
                    .mutability()
                    .is_mut()
                    .then(|| (name, global.get(&self.session.store)))
            })
            .collect();
        self.baseline = Some(Baseline {
            memory: self.session.memory.data(&self.session.store).to_vec(),
            globals,
            manifest: self.session.manifest.clone(),
            registered_sources: self.session.registered_sources,
            emitted_debug: self.session.emitted_debug.get(),
        });
        Ok(())
    }

    /// Completed calls truncate the language arenas in the existing instance.
    /// A trapped/poisoned instance is restored from the initialization snapshot.
    pub fn reset(&mut self) -> Result<(), String> {
        let baseline = self.baseline.as_ref().ok_or("service is not initialized")?;
        let limit = baseline
            .memory
            .len()
            .checked_add(self.session.memory_limit)
            .ok_or("service memory limit overflow")?;
        if !self.poisoned {
            // Cleanup is not charged to the next request, and must not inherit
            // the previous request's nearly exhausted fuel.
            self.session.store.set_fuel(self.session.fuel_budget).map_err(|e| e.to_string())?;
            if self.session.exports.reset_service.call(&mut self.session.store, ()).is_ok() {
                for (name, value) in &baseline.globals {
                    self.session.instance.get_global(&self.session.store, name)
                        .ok_or("missing reset global")?
                        .set(&mut self.session.store, value.clone()).map_err(|e| e.to_string())?;
                }
                *self.session.store.data_mut() = wasmi::StoreLimitsBuilder::new()
                    .memory_size(limit).table_elements(1_000_000).trap_on_grow_failure(true).build();
                self.session.store.set_fuel(self.session.fuel_budget).map_err(|e| e.to_string())?;
                self.session.emitted_debug.set(baseline.emitted_debug);
                return Ok(());
            }
            self.poisoned = true;
        }
        let engine = self.session.module.engine();
        let limits = wasmi::StoreLimitsBuilder::new()
            .memory_size(limit)
            .table_elements(1_000_000)
            .trap_on_grow_failure(true)
            .build();
        let mut store = wasmi::Store::new(engine, limits);
        store.limiter(|limits| limits);
        store
            .set_fuel(self.session.fuel_budget)
            .map_err(|e| e.to_string())?;
        let instance = wasmi::Linker::new(engine)
            .instantiate_and_start(&mut store, &self.session.module)
            .map_err(|e| e.to_string())?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("missing service memory")?;
        let current = memory.data_size(&store);
        if baseline.memory.len() > current {
            memory
                .grow(
                    &mut store,
                    ((baseline.memory.len() - current) / 65536) as u64,
                )
                .map_err(|e| e.to_string())?;
        }
        memory
            .write(&mut store, 0, &baseline.memory)
            .map_err(|e| e.to_string())?;
        for (name, value) in &baseline.globals {
            instance
                .get_global(&store, name)
                .ok_or("missing reset global")?
                .set(&mut store, value.clone())
                .map_err(|e| e.to_string())?;
        }
        store
            .set_fuel(self.session.fuel_budget)
            .map_err(|e| e.to_string())?;
        let exports = crate::session::exports::Exports::bind(instance, &store)?;
        self.session.exports = exports;
        self.session.store = store;
        self.session.instance = instance;
        self.session.memory = memory;
        self.session.manifest = baseline.manifest.clone();
        self.session.registered_sources = baseline.registered_sources;
        self.session.emitted_debug.set(baseline.emitted_debug);
        self.poisoned = false;
        Ok(())
    }

    pub fn transform(&mut self, input: &[u8]) -> Result<Vec<u8>, String> {
        if !self.ready || self.poisoned {
            return Err("service requires initialization or reset".into());
        }
        self.poisoned = true;
        let length = u32::try_from(input.len()).map_err(|_| "service request exceeds wasm32")?;
        let pointer = buffers::alloc(&mut self.session, length, 1)?;
        self.session
            .memory
            .write(&mut self.session.store, pointer as usize, input)
            .map_err(|e| e.to_string())?;
        let result = buffers::alloc(&mut self.session, 12, 4)?;
        self.session.exports.run_service
            .call(&mut self.session.store, (pointer, length, 1, 0, result))
            .map_err(|e| e.to_string())?;
        // No free after a trap: the next reset discards the entire failed instance.
        buffers::free(&mut self.session, pointer, length, 1)?;
        let response = buffers::response(&mut self.session, result)?;
        self.poisoned = false;
        Ok(response)
    }
}
