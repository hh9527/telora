//! One static service entry, driven once or through the shared serving transports.
use super::*;
use std::io::Write;
use telora_wasm::transform_service::TransformSession;

pub(crate) fn execute(
    context: PathBuf,
    arguments: crate::ApplicationArgs,
    bind: Option<telora_run::transport::Bind>,
) -> Result<i32, String> {
    let frontend = PhaseTimer::new("frontend");
    if arguments.module.contains(':') {
        return Err("service entry expects MODULE, exporting MainService".into());
    }
    let mut inventory = Inventory::new(&context, arguments.module.starts_with("std/"))?;
    let module = inventory.select(&arguments.module)?;
    let mut mir = inventory.solve_transform(&module)?;
    if mir
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return Err(mir
            .diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let sealed = mir.seal().map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let ModuleTarget::Bound(root) = mir.roots[0] else {
        return Err("unresolved service entry".into());
    };
    let symbol = *mir.exports[root.index()]
        .iter()
        .find(|s| mir.symbols[s.index()].name == "main")
        .ok_or("missing static service plan")?;
    let executable = sealed.seal_export(symbol).map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    drop(frontend);
    let bytes = {
        let _timer = PhaseTimer::new("codegen_link");
        telora_wasm::compile_service(&executable)?
    };
    let mut session = {
        let _timer = PhaseTimer::new("engine_load");
        load_session(&bytes, inventory.runtime_options())?
    };
    initialize(&mut session, &inventory, &mut mir.sources)?;
    let service_init = PhaseTimer::new("service_initialize");
    let mut service = TransformSession::new(session)?;
    let names = crate::source_arg::service_source_names(&arguments.sources)?;
    if names != service.sources() {
        return Err(format!(
            "service sources differ: declared {:?}, supplied {names:?}",
            service.sources()
        ));
    }
    crate::source_arg::reject_stdin_sources(&arguments.sources)?;
    let limits = crate::execution_config_for(inventory.runtime_options())?.data_limits;
    let sources = crate::source_arg::service_source_readers(arguments.sources);
    let initialization = service.initialize_readers(sources, limits.file_size)?;
    for diagnostic in initialization.diagnostics.as_array().into_iter().flatten() {
        emit_diagnostic(diagnostic)?;
    }
    for event in service.session().take_debug_events()? {
        crate::emit_stderr(serde_json::to_value(event).map_err(|e| e.to_string())?)?;
    }
    if !initialization.success {
        return Ok(1);
    }
    service.seal_initialization()?;
    timing::initialization_heap(service.session_mut())?;
    drop(service_init);
    let usage_reporter = service.session_mut().usage_reporter.take();
    let stdin = std::io::stdin();
    if bind.is_none() {
        let input = crate::source_arg::read_limited(stdin.lock(), limits.file_size, "query input")?;
        let response = transform(&mut service, &input);
        if let Some(report) = usage_reporter {
            report(service.usage());
        }
        let reply: Reply<'_> = serde_json::from_slice(&response).map_err(|e| e.to_string())?;
        if reply.schema != "telora.service/v1" {
            return Err("invalid service response schema".into());
        }
        for diagnostic in &reply.diagnostics {
            emit_diagnostic(diagnostic)?;
        }
        if reply.error {
            return Ok(1);
        }
        write_json(reply.ok.get().as_bytes())?;
        return Ok(0);
    }
    telora_run::transport::serve(bind.unwrap(), limits.file_size, |input| {
        let response = transform(&mut service, input);
        if let Some(report) = usage_reporter {
            report(service.usage());
        }
        Ok(response)
    })
    .map_err(|e| e.to_string())?;
    Ok(0)
}

#[derive(serde::Deserialize)]
struct Reply<'a> {
    schema: &'a str,
    #[serde(borrow)]
    ok: &'a serde_json::value::RawValue,
    error: bool,
    diagnostics: Vec<serde_json::Value>,
}

fn write_json(bytes: &[u8]) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    out.write_all(bytes)
        .and_then(|_| out.write_all(b"\n"))
        .map_err(|e| e.to_string())
}

fn transform(service: &mut TransformSession, input: &[u8]) -> Vec<u8> {
    let result = (|| {
        {
            let _timer = PhaseTimer::new("request_reset");
            service.reset()?;
        }
        let _timer = PhaseTimer::new("request_transform");
        let result = service.transform(input);
        for event in service.session().take_debug_events()? {
            crate::emit_stderr(serde_json::to_value(event).map_err(|e| e.to_string())?)?;
        }
        result
    })();
    match result {
        Ok(response) => response,
        Err(error) => failure_bytes(&error),
    }
}

fn failure_bytes(message: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"schema":"telora.service/v1","ok":null,"error":true,"diagnostics":[{
            "severity":"Error", "message":message, "labels":[], "notes":[]
        }]}),
    )
    .expect("serializable service failure")
}

fn emit_diagnostic(diagnostic: &serde_json::Value) -> Result<(), String> {
    crate::emit_stderr(
        serde_json::json!({"schema":"telora.execution/v1", "record":"diagnostic",
        "severity":diagnostic["severity"], "message":diagnostic["message"],
        "labels":diagnostic["labels"], "notes":diagnostic["notes"]}),
    )
}
