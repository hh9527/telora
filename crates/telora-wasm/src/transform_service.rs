//! Byte-only service host. Guest owns Context, initialization and the handler.
use crate::{artifact::Manifest, session::Session};
use telora_core::data_plan::Format;
mod buffers;
mod input;

struct Baseline {
    snapshot: Vec<u8>,
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
    pub fn new(mut session: Session) -> Result<Self, String> {
        let count = session
            .exports
            .source_count
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
        self.session.usage()
    }

    pub fn initialize(&mut self, sources: &[SourceInput<'_>]) -> Result<Initialization, String> {
        self.initialize_readers(
            sources.iter().map(|source| {
                Ok(SourceReader {
                    name: source.name.to_owned(),
                    reader: Box::new(std::io::Cursor::new(source.data)),
                    format: source.format,
                })
            }),
            u32::MAX as usize,
        )
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
            let length = buffer
                .read(&mut self.session, &mut source.reader, max_bytes)
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
        let status = self
            .session
            .exports
            .create_service
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
        self.session
            .exports
            .diagnostics
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

    /// All Host-owned ABI buffers have been freed before sealing initialization or reset.
    pub fn seal_initialization(&mut self) -> Result<(), String> {
        if self.baseline.is_some() {
            return Err("service initialization is already sealed".into());
        }
        if !self.ready || self.poisoned {
            return Err("service is not initialized".into());
        }
        let globals: Vec<(String, wasmi::Val)> = self
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
        let snapshot = export_snapshot(&mut self.session)?;
        let mut compact = self.session.fresh_instance()?;
        import_snapshot(&mut compact, &snapshot)?;
        for (name, value) in &globals {
            compact
                .instance
                .get_global(&compact.store, name)
                .ok_or("missing reset global")?
                .set(&mut compact.store, value.clone())
                .map_err(|error| error.to_string())?;
        }
        compact.usage_reporter = self.session.usage_reporter.take();
        self.session = compact;
        self.baseline = Some(Baseline {
            snapshot,
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
        let limit = self.session.memory_limit;
        if !self.poisoned {
            // Cleanup is not charged to the next request, and must not inherit
            // the previous request's nearly exhausted fuel.
            self.session
                .store
                .set_fuel(self.session.fuel_budget)
                .map_err(|e| e.to_string())?;
            if self
                .session
                .exports
                .reset_service
                .call(&mut self.session.store, ())
                .is_ok()
            {
                for (name, value) in &baseline.globals {
                    self.session
                        .instance
                        .get_global(&self.session.store, name)
                        .ok_or("missing reset global")?
                        .set(&mut self.session.store, value.clone())
                        .map_err(|e| e.to_string())?;
                }
                *self.session.store.data_mut() = wasmi::StoreLimitsBuilder::new()
                    .memory_size(limit)
                    .table_elements(1_000_000)
                    .trap_on_grow_failure(true)
                    .build();
                self.session
                    .store
                    .set_fuel(self.session.fuel_budget)
                    .map_err(|e| e.to_string())?;
                self.session.emitted_debug.set(baseline.emitted_debug);
                return Ok(());
            }
            self.poisoned = true;
        }
        let mut compact = self.session.fresh_instance()?;
        import_snapshot(&mut compact, &baseline.snapshot)?;
        for (name, value) in &baseline.globals {
            compact
                .instance
                .get_global(&compact.store, name)
                .ok_or("missing reset global")?
                .set(&mut compact.store, value.clone())
                .map_err(|e| e.to_string())?;
        }
        compact
            .store
            .set_fuel(self.session.fuel_budget)
            .map_err(|e| e.to_string())?;
        compact.manifest = baseline.manifest.clone();
        compact.registered_sources = baseline.registered_sources;
        compact.emitted_debug.set(baseline.emitted_debug);
        compact.usage_reporter = self.session.usage_reporter.take();
        self.session = compact;
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
        self.session
            .exports
            .run_service
            .call(&mut self.session.store, (pointer, length, 1, 0, result))
            .map_err(|e| e.to_string())?;
        // No free after a trap: the next reset discards the entire failed instance.
        buffers::free(&mut self.session, pointer, length, 1)?;
        let response = buffers::response(&mut self.session, result)?;
        self.poisoned = false;
        Ok(response)
    }
}

fn export_snapshot(session: &mut Session) -> Result<Vec<u8>, String> {
    let result = buffers::alloc(session, 12, 4)?;
    session
        .exports
        .snapshot_export
        .call(&mut session.store, result)
        .map_err(|error| error.to_string())?;
    let [pointer, length, _] = buffers::words(session, result)?;
    let bytes = buffers::bytes(session, pointer, length)?;
    buffers::free(session, pointer, length, 1)?;
    buffers::free(session, result, 12, 4)?;
    Ok(bytes)
}

fn import_snapshot(session: &mut Session, snapshot: &[u8]) -> Result<(), String> {
    let length = u32::try_from(snapshot.len()).map_err(|_| "service snapshot exceeds wasm32")?;
    let pointer = buffers::alloc(session, length, 1)?;
    session
        .memory
        .write(&mut session.store, pointer as usize, snapshot)
        .map_err(|error| error.to_string())?;
    session
        .exports
        .snapshot_import
        .call(&mut session.store, (pointer, length))
        .map_err(|error| error.to_string())?;
    buffers::free(session, pointer, length, 1)
}
