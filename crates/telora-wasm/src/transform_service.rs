//! A closed init/transform entry. Reset restores bytes, never host-owned values.
use crate::{artifact::Manifest, session::Session, transport::Value};

struct Baseline {
    memory: Vec<u8>,
    globals: Vec<(String, wasmi::Val)>,
    manifest: Manifest,
    registered_sources: usize,
    emitted_debug: u32,
}

pub struct TransformSession {
    session: Session,
    initializer: Value,
    handler: Option<Value>,
    baseline: Option<Baseline>,
    sources: Vec<String>,
}

impl TransformSession {
    /// Called after normal module initialization. The compiler-owned Plan is
    /// (source names, Context -> (Value -> String)), with all types sealed.
    pub fn new(mut session: Session) -> Result<Self, String> {
        let plan = Value {pointer: session.entry()?, ty: session.manifest.entry_type};
        let (sources, initializer) = session.pair(plan)?;
        let sources = serde_json::from_value(session.output_value(sources)?)
            .map_err(|e| format!("invalid sealed source list: {e}"))?;
        Ok(Self {session, initializer, sources, handler: None, baseline: None})
    }

    pub fn sources(&self) -> &[String] { &self.sources }
    pub fn session(&self) -> &Session { &self.session }
    pub fn session_mut(&mut self) -> &mut Session { &mut self.session }

    pub fn usage(&self) -> crate::session::Usage {
        let mut usage = self.session.usage();
        if let Some(baseline) = &self.baseline {
            usage.memory_limit = usage.memory_limit.saturating_add(baseline.memory.len());
        }
        usage
    }

    pub fn initialize(&mut self, sources: &std::collections::BTreeMap<String, Value>) -> Result<(), String> {
        if self.handler.is_some() { return Err("service is already initialized".into()); }
        if self.sources.iter().ne(sources.keys()) {
            return Err("service sources do not match declared sources".into());
        }
        let ctx_ty = self.session.manifest.types[self.initializer.ty as usize].arguments[0];
        let dict_ty = self.session.manifest.types[ctx_ty as usize].fields[0].ty;
        let string_ty = self.session.manifest.types.iter().position(|ty|
            ty.kind == crate::artifact::Kind::String).ok_or("missing String layout")? as u32;
        let mut values = vec![];
        for (name, &value) in sources {
            let key = self.session.input_value(string_ty, &name.clone().into())?;
            values.push((key.pointer, value.pointer));
        }
        let dict = self.session.input_dict_values(dict_ty, &values)?;
        let ctx = self.session.input_record_values(ctx_ty,
            &std::collections::BTreeMap::from([("sources", dict)]))?;
        let handler = self.session.invoke_values(self.initializer, &[Value {pointer: ctx, ty: ctx_ty}])?;
        self.handler = Some(handler);
        Ok(())
    }

    /// The host consumes initialization diagnostics before fixing the baseline.
    pub fn seal_initialization(&mut self) -> Result<(), String> {
        if self.baseline.is_some() { return Err("service initialization is already sealed".into()); }
        let handler = self.handler.ok_or("service is not initialized")?;
        // Module evaluation is not the end of service initialization. Freeze
        // injected source records together with the completed service graph,
        // before any collector can reuse their arena storage.
        let freeze = self.session.instance
            .get_typed_func::<(), u32>(&self.session.store, "telora_freeze")
            .map_err(|e| e.to_string())?;
        freeze.call(&mut self.session.store, ()).map_err(|e| e.to_string())?;
        let (roots, _) = self.session.collect_work(&[handler])?;
        self.handler = Some(roots[0]);
        let globals = self.session.instance.exports(&self.session.store)
            .filter_map(|export| {
                let name = export.name().to_owned();
                if !name.starts_with("telora_reset_global_") { return None; }
                let global = export.into_global()?;
                global.ty(&self.session.store).mutability().is_mut()
                    .then(|| (name, global.get(&self.session.store)))
            }).collect();
        self.baseline = Some(Baseline {
            memory: self.session.memory.data(&self.session.store).to_vec(),
            globals,
            manifest: self.session.manifest.clone(),
            registered_sources: self.session.registered_sources,
            emitted_debug: self.session.emitted_debug.get(),
        });
        Ok(())
    }

    /// Fresh engine store, same compiled module and initialized memory. No
    /// codegen, source reads, module initialization or user init is repeated.
    pub fn reset(&mut self) -> Result<(), String> {
        let baseline = self.baseline.as_ref().ok_or("service is not initialized")?;
        let engine = self.session.module.engine();
        let limit = baseline.memory.len().checked_add(self.session.memory_limit)
            .ok_or("service memory limit overflow")?;
        let limits = wasmi::StoreLimitsBuilder::new().memory_size(limit)
            .table_elements(1_000_000).trap_on_grow_failure(true).build();
        let mut store = wasmi::Store::new(engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(self.session.fuel_budget).map_err(|e| e.to_string())?;
        let instance = wasmi::Linker::new(engine)
            .instantiate_and_start(&mut store, &self.session.module).map_err(|e| e.to_string())?;
        let memory = instance.get_memory(&store, "memory").ok_or("missing service memory")?;
        let current = memory.data_size(&store);
        if baseline.memory.len() > current {
            memory.grow(&mut store, ((baseline.memory.len() - current) / 65536) as u64)
                .map_err(|e| e.to_string())?;
        }
        memory.write(&mut store, 0, &baseline.memory).map_err(|e| e.to_string())?;
        for (name, value) in &baseline.globals {
            instance.get_global(&store, name).ok_or("missing reset global")?
                .set(&mut store, value.clone()).map_err(|e| e.to_string())?;
        }
        store.set_fuel(self.session.fuel_budget).map_err(|e| e.to_string())?;
        self.session.store = store;
        self.session.instance = instance;
        self.session.memory = memory;
        self.session.manifest = baseline.manifest.clone();
        self.session.registered_sources = baseline.registered_sources;
        self.session.emitted_debug.set(baseline.emitted_debug);
        Ok(())
    }

    /// Input is materialized after reset. Language errors are serialized by the Guest entry;
    /// engine traps return an outer error and the next reset discards the store.
    pub fn transform(&mut self, input: Value) -> Result<serde_json::Value, String> {
        let handler = self.handler.ok_or("service is not initialized")?;
        let output = self.session.invoke_values(handler, &[input])?;
        let text = self.session.output_value(output)?;
        let text = text.as_str().ok_or("service entry did not return serialized JSON")?;
        serde_json::from_str(text).map_err(|e| format!("invalid service response: {e}"))
    }
}
