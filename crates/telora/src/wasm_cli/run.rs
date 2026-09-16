//! One static service entry, driven once or by a JSONL input stream.
use super::*;
use std::io::{BufRead, Write};
use telora_core::data_plan::Format;
use telora_wasm::transform_service::{SourceInput, TransformSession};

pub(crate) fn execute(
    context: PathBuf,
    arguments: crate::ApplicationArgs,
    serve: bool,
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
    let sources = crate::source_arg::collect_service_sources(arguments.sources, limits.file_size)?;
    let inputs = sources
        .iter()
        .map(|(name, input)| SourceInput {
            name,
            data: input.text.as_bytes(),
            format: match input.format {
                telora_core::SystemDataFormat::Json => Format::Json,
                telora_core::SystemDataFormat::Yaml => Format::Yaml,
                telora_core::SystemDataFormat::Toml => Format::Toml,
            },
        })
        .collect::<Vec<_>>();
    let initialization = service.initialize(&inputs)?;
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
    drop(service_init);
    let usage_reporter = service.session_mut().usage_reporter.take();
    let stdin = std::io::stdin();
    if !serve {
        let input = crate::source_arg::read_limited(stdin.lock(), limits.file_size, "query input")?;
        let response = transform(&mut service, &input);
        if let Some(report) = usage_reporter {
            report(service.usage());
        }
        for diagnostic in response["diagnostics"].as_array().into_iter().flatten() {
            emit_diagnostic(diagnostic)?;
        }
        if response["error"] == true {
            return Ok(1);
        }
        crate::emit(response["ok"].clone())?;
        return Ok(0);
    }
    let mut reader = stdin.lock();
    loop {
        let mut bytes = Vec::new();
        let mut oversized = false;
        loop {
            let available = reader.fill_buf().map_err(|e| e.to_string())?;
            if available.is_empty() {
                break;
            }
            let end = available.iter().position(|b| *b == b'\n').map(|n| n + 1);
            let count = end.unwrap_or(available.len());
            if bytes.len().saturating_add(count) <= limits.file_size {
                bytes.extend_from_slice(&available[..count]);
            } else {
                oversized = true;
            }
            reader.consume(count);
            if end.is_some() {
                break;
            }
        }
        if bytes.is_empty() && !oversized {
            break;
        }
        let response = if oversized {
            failure("request exceeds file_size limit")
        } else {
            let response = transform(&mut service, &bytes);
            if let Some(report) = usage_reporter {
                report(service.usage());
            }
            response
        };
        crate::emit(response)?;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    Ok(0)
}

fn transform(service: &mut TransformSession, input: &[u8]) -> serde_json::Value {
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
        Err(error) => failure(&error),
    }
}

fn failure(message: &str) -> serde_json::Value {
    serde_json::json!({"schema":"telora.service/v1","ok":null,"error":true,"diagnostics":[{
        "severity":"Error", "message":message, "labels":[], "notes":[]
    }]})
}

fn emit_diagnostic(diagnostic: &serde_json::Value) -> Result<(), String> {
    crate::emit_stderr(
        serde_json::json!({"schema":"telora.execution/v1", "record":"diagnostic",
        "severity":diagnostic["severity"], "message":diagnostic["message"],
        "labels":diagnostic["labels"], "notes":diagnostic["notes"]}),
    )
}
