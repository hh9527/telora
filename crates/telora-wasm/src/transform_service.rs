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
    debug_events: u32,
    recovery_fuel: u64,
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
        let count = session.guest_call(session.exports.source_count, ())?;
        let result = buffers::alloc(&mut session, 12, 4)?;
        let get = session.exports.source_name;
        let mut sources = Vec::new();
        let mut source_ids = Vec::new();
        for index in 0..count {
            session.guest_call(get, (index, result))?;
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

    pub fn publication_snapshot(
        &self,
    ) -> Result<telora_wasm_shared::snapshot_artifact::Snapshot, String> {
        use telora_wasm_shared::snapshot_artifact::{GlobalValue, Snapshot};
        let baseline = self
            .baseline
            .as_ref()
            .ok_or("service initialization is not sealed")?;
        let mut globals = Vec::with_capacity(baseline.globals.len());
        for (name, value) in &baseline.globals {
            let value = match value {
                wasmi::Val::I32(value) => GlobalValue::I32(*value),
                wasmi::Val::I64(value) => GlobalValue::I64(*value),
                wasmi::Val::F32(value) => GlobalValue::F32(value.to_bits()),
                wasmi::Val::F64(value) => GlobalValue::F64(value.to_bits()),
                _ => return Err("snapshot reset globals must be scalar".into()),
            };
            globals.push((name.clone(), value));
        }
        globals.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(Snapshot {
            guest: baseline.snapshot.clone(),
            globals,
        })
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
        self.session
            .initialization_quota
            .get_or_insert_with(|| crate::fuel_quota::FuelQuota::new(self.session.fuel_budget));
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
            self.session
                .initialization_quota
                .as_mut()
                .unwrap()
                .call(
                    &mut self.session.store,
                    set,
                    (id, buffer.pointer, length, format),
                )
                .map_err(|error| self.session.allocation_error(error))?;
            supplied += 1;
        }
        buffer.free(&mut self.session)?;
        if supplied != self.sources.len() {
            return Err("service sources do not match declared sources".into());
        }
        let status = self
            .session
            .initialization_quota
            .as_mut()
            .unwrap()
            .call(
                &mut self.session.store,
                self.session.exports.create_service,
                (),
            )
            .map_err(|error| self.session.allocation_error(error))?;
        let diagnostics = self.initialization_diagnostics()?;
        self.ready = status == 0;
        self.session.metered_fuel = self
            .session
            .initialization_quota
            .as_ref()
            .map(|quota| quota.consumed());
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
            .guest_call(self.session.exports.diagnostics, (1, 0, result))?;
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
        self.seal_initialization_mode(false).map(|_| ())
    }

    pub(crate) fn seal_runtime_initialization(
        &mut self,
    ) -> Result<crate::specialization::RuntimeDependencies, String> {
        self.seal_initialization_mode(true)?
            .ok_or_else(|| "missing runtime publication dependencies".into())
    }

    fn seal_initialization_mode(
        &mut self,
        runtime_only: bool,
    ) -> Result<Option<crate::specialization::RuntimeDependencies>, String> {
        if self.baseline.is_some() {
            return Err("service initialization is already sealed".into());
        }
        if !self.ready || self.poisoned {
            return Err("service is not initialized".into());
        }
        self.session.initialization_quota = None;
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
        let (snapshot, dependencies) = export_snapshot_mode(&mut self.session, runtime_only)?;
        self.baseline = Some(Baseline {
            snapshot,
            globals,
            manifest: self.session.manifest.clone(),
            registered_sources: self.session.registered_sources,
            debug_events: self
                .session
                .output()
                .word(crate::abi::table_address(crate::abi::DEBUG_EVENTS) as u64 + 4)?,
            recovery_fuel: self.session.fuel_budget,
        });
        self.session.start_execution()?;
        Ok(dependencies)
    }

    /// Completed calls release invocation roots and execution state. Dynamic
    /// garbage is collected by the allocation policy; traps restore the actor.
    pub fn reset(&mut self) -> Result<(), String> {
        let baseline = self.baseline.as_ref().ok_or("service is not initialized")?;
        let limit = self.session.memory_limit;
        if !self.poisoned && self.session.ensure_live().is_ok() {
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
                self.session.emitted_debug.set(0);
                self.session.metered_fuel = None;
                return Ok(());
            }
            self.poisoned = true;
        }
        let mut compact = self.session.fresh_instance()?;
        // Relocation is recovery work, bounded by the initialization budget.
        // A small per-request budget must remain available for the next call.
        compact
            .store
            .set_fuel(baseline.recovery_fuel)
            .map_err(|error| error.to_string())?;
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
        compact
            .emitted_debug
            .set(self.session.emitted_debug.get().min(baseline.debug_events));
        compact.metered_fuel = None;
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
        let mut quota = crate::fuel_quota::FuelQuota::new(self.session.fuel_budget);
        let called = quota.call(
            &mut self.session.store,
            self.session.exports.run_service,
            (pointer, length, 1, 0, result),
        );
        self.session.metered_fuel = Some(quota.consumed());
        self.session.trapped |= called.is_err();
        called.map_err(|error| {
            self.session
                .allocation_error_with_growth(error, quota.growth_failure())
        })?;
        // No free after a trap: the next reset discards the entire failed instance.
        buffers::free(&mut self.session, pointer, length, 1)?;
        let response = buffers::response(&mut self.session, result)?;
        self.poisoned = false;
        Ok(response)
    }
}

fn export_snapshot_mode(
    session: &mut Session,
    runtime_only: bool,
) -> Result<(Vec<u8>, Option<crate::specialization::RuntimeDependencies>), String> {
    if !runtime_only {
        let result = buffers::alloc(session, 12, 4)?;
        session.guest_call(session.exports.snapshot_export, result)?;
        let [pointer, length, _] = buffers::words(session, result)?;
        let snapshot = buffers::bytes(session, pointer, length)?;
        buffers::free(session, pointer, length, 1)?;
        buffers::free(session, result, 12, 4)?;
        return Ok((snapshot, None));
    }
    let export = session
        .instance
        .get_typed_func::<u32, ()>(&session.store, "telora_snapshot_runtime_export")
        .map_err(|e| e.to_string())?;
    let result = buffers::alloc(session, 24, 4)?;
    session.guest_call(export, result)?;
    let fields = buffers::bytes(session, result, 24)?;
    let mut values = Vec::with_capacity(3);
    for pair in fields.chunks_exact(8) {
        let pointer = u32::from_le_bytes(pair[..4].try_into().unwrap());
        let length = u32::from_le_bytes(pair[4..].try_into().unwrap());
        values.push(buffers::bytes(session, pointer, length)?);
        buffers::free(session, pointer, length, 1)?;
    }
    buffers::free(session, result, 24, 4)?;
    let indices = |bytes: &[u8]| -> Result<Vec<u32>, String> {
        if bytes.len() % 4 != 0 {
            return Err("invalid runtime dependency transfer".into());
        }
        Ok(bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect())
    };
    let dependencies = crate::specialization::RuntimeDependencies {
        functions: indices(&values[1])?,
        ready_demands: indices(&values[2])?.into_iter().collect(),
    };
    Ok((values.remove(0), Some(dependencies)))
}

fn import_snapshot(session: &mut Session, snapshot: &[u8]) -> Result<(), String> {
    let length = u32::try_from(snapshot.len()).map_err(|_| "service snapshot exceeds wasm32")?;
    let pointer = buffers::alloc(session, length, 1)?;
    session
        .memory
        .write(&mut session.store, pointer as usize, snapshot)
        .map_err(|error| error.to_string())?;
    session.guest_call(session.exports.snapshot_import, (pointer, length))?;
    buffers::free(session, pointer, length, 1)
}
