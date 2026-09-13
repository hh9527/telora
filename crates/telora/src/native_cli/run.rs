use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use telora_core::{entry_plan::{self, RunMode}, mir::{Mir, SymbolId, TypeState}, RunHost, SystemEvent};
use telora_native::{runtime::{DataContract, ServiceEffect}, service::ServiceSession};

#[derive(Default)]
struct ServiceMetrics {
    events: u64,
    collections: u64,
    reduce_ns: u128,
    collect_ns: u128,
    copied_bytes: u64,
    max_objects_before: usize,
    max_objects_after: usize,
}
impl Drop for ServiceMetrics {
    fn drop(&mut self) {
        if std::env::var_os("TELORA_NATIVE_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1")) {
            eprintln!("{}", serde_json::json!({"native_phase":"service", "events":self.events,
                "collections":self.collections, "reduce_ns":self.reduce_ns, "collect_ns":self.collect_ns,
                "copied_bytes":self.copied_bytes, "max_objects_before":self.max_objects_before,
                "max_objects_after":self.max_objects_after}));
        }
    }
}

pub(crate) async fn execute(mut mir: Mir, inventory: Inventory, symbol: SymbolId, mode: RunMode,
    arguments: crate::ApplicationArgs, inputs: crate::source_arg::CollectedEntrySources) -> Result<i32, String> {
    let sealed = mir.seal().map_err(|ds| ds.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else { return Err("unclosed service signature".into()); };
    let contract = entry_plan::run_contract(sealed.types(), ty).ok_or("service requires a closed policy signature")?;
    let data = DataContract::from_mir(&sealed)?;
    let executable = sealed.seal_export(symbol).map_err(|ds| ds.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    let mut session = Session::compile_executable(&executable)?;
    let reports = session.initialize(&inventory, &mut mir.sources);
    if reports.iter().any(|d| d.severity == Severity::Error) {
        return Err(reports.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"));
    }
    for report in &reports { eprintln!("{}", mir.sources.render(report)); }
    let configure = session.compiled.export(&mut session.context, symbol)?;
    let mut service = ServiceSession::new(session.compiled, session.context, configure, contract)?;
    let report_start = service.context().diagnostics().len();
    let mut host = crate::ProcessRunHost::new(inputs.locators, arguments.ees_vars);
    let result = execute_inner(&mut service, &data, &mut mir.sources, mode, &arguments.args, &inputs.entry, &mut host).await;
    let finished = host.finish().await;
    let reports = context_diagnostics(service.context(), &mir.sources).into_iter().skip(report_start).collect::<Vec<_>>();
    let result = result.map_err(|error| {
        if reports.iter().any(|d| d.severity == Severity::Error) { reports.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n") }
        else { error }
    });
    for report in reports.iter().filter(|d| d.severity != Severity::Error) { eprintln!("{}", mir.sources.render(report)); }
    let (output, code) = match (result, finished) {
        (Ok(result), Ok(())) => result,
        (_, Err(error)) | (Err(error), Ok(())) => return Err(error),
    };
    std::io::stdout().write_all(output.as_bytes()).and_then(|()| std::io::stdout().flush())
        .map_err(|error| format!("cannot write Entry output: {error}"))?;
    i32::try_from(code).map_err(|_| format!("Entry exit status {code} is outside the Host range"))
}

fn input(service: &mut ServiceSession, contract: &DataContract, sources: &mut SourceDatabase,
    name: String, format: telora_core::SystemDataFormat, text: &str) -> Result<telora_native::abi::Value, String> {
    let limits = crate::execution_config().data_limits;
    let source = sources.try_add(name, text).map_err(|e| e.to_string())?;
    let format = match format { telora_core::SystemDataFormat::Json => telora_core::data_plan::Format::Json,
        telora_core::SystemDataFormat::Yaml => telora_core::data_plan::Format::Yaml, telora_core::SystemDataFormat::Toml => telora_core::data_plan::Format::Toml };
    let plan = telora_core::data_plan::parse_registered(sources, source, format)
        .map_err(|ds| ds.iter().map(|d| sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    telora_core::data_plan::enforce_limits(&plan, limits, text.len())
        .map_err(|message| sources.render(&Diagnostic::error(message, telora_core::Loc { source, start: 0, end: 0 })))?;
    let rt = service.context_mut().runtime_mut()?;
    rt.register_source_names(sources);
    rt.materialize_data(contract, &plan)
}

async fn execute_inner(service: &mut ServiceSession, data: &DataContract, sources: &mut SourceDatabase, mode: RunMode,
    args: &[String], inputs: &telora_core::EntryDataSources, host: &mut crate::ProcessRunHost) -> Result<(String, i64), String> {
    let setup_timer = PhaseTimer::new("service_setup");
    let mut metrics = ServiceMetrics::default();
    let contract = service.contract();
    let env = service.context_mut().runtime_mut()?.service_env(contract, mode, args, inputs, &host.ees_actors())?;
    let caps_value = service.configure(env)?;
    let caps = service.context().runtime()?.service_caps(&caps_value, data)?;
    host.configure(caps.clone()).await.map_err(|e| format!("cannot satisfy Entry capabilities: {e}"))?;
    let mut prepared = BTreeMap::new();
    for (name, request) in &caps.data_sources {
        if let Some(text) = host.read_data_source(request, crate::execution_config().data_limits.file_size).await? {
            prepared.insert(name.clone(), input(service, data, sources, request.src.clone(), request.format, &text)?);
        }
    }
    let mut texts = BTreeMap::new();
    for (name, request) in &caps.text_sources {
        let text = match std::fs::read_to_string(&request.src) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => request.default.clone().ok_or_else(|| format!("cannot read text source {:?}: {error}", request.src))?,
            Err(error) => return Err(format!("cannot read text source {:?}: {error}", request.src)),
        };
        texts.insert(name.clone(), text);
    }
    let mut vars = BTreeMap::new();
    for name in &caps.vars {
        match std::env::var(name) {
            Ok(value) => { vars.insert(name.clone(), value); },
            Err(std::env::VarError::NotPresent) => {},
            Err(error) => return Err(format!("cannot read variable {name:?}: {error}")),
        }
    }
    let stdin = if caps.stdin == telora_core::SystemStdin::Text {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("cannot read standard input: {e}"))?;
        Some(text)
    } else { None };
    let resources = service.context_mut().runtime_mut()?.service_resources(contract, &caps_value, prepared, &texts, &vars, stdin.as_deref())?;
    service.initialize(resources)?;
    drop(setup_timer);
    let mut next = None;
    let mut output = String::new();
    loop {
        let reply = if let Some(SystemEvent::EesReply(reply)) = &next {
            match &reply.result {
                Ok(value) => Some(input(service, data, sources, "<EES reply>".into(), telora_core::SystemDataFormat::Json, &value.to_string())?),
                Err(_) => None,
            }
        } else { None };
        let event = service.context_mut().runtime_mut()?.service_event(contract, next, reply)?;
        let started = std::time::Instant::now();
        let effects = service.reduce(event);
        metrics.events += 1;
        metrics.reduce_ns += started.elapsed().as_nanos();
        let effects = effects?;
        // Fully validate this transition before any external effect occurs.
        let effects = service.context().runtime()?.service_effects(&effects, &caps, data)?;
        for effect in effects {
            match effect {
                ServiceEffect::Output(text) => output.push_str(&text),
                ServiceEffect::EesCall(call) => host.ees_call(call).await?,
                ServiceEffect::Exit(code) => return Ok((output, code)),
            }
        }
        let started = std::time::Instant::now();
        let (_, collected) = service.collect(&[])?;
        metrics.collections += 1;
        metrics.collect_ns += started.elapsed().as_nanos();
        metrics.copied_bytes += collected.copied_bytes;
        metrics.max_objects_before = metrics.max_objects_before.max(collected.objects_before);
        metrics.max_objects_after = metrics.max_objects_after.max(collected.objects_after);
        next = Some(host.next_event().await?.ok_or("Entry made no progress and the Host has no pending event")?);
    }
}
