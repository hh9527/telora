//! One static service entry, driven once or by a JSONL input stream.
use super::*;
use std::{collections::BTreeMap, io::{BufRead, Write}};
use telora_core::{SourceDatabase, SourceId, data_plan::{self, Format}};
use telora_wasm::{transform_service::TransformSession, transport::Value};

pub(crate) fn execute(context: PathBuf, arguments: crate::ApplicationArgs, serve: bool) -> Result<i32, String> {
    if arguments.module.contains(':') { return Err("service entry expects MODULE, exporting MainService".into()); }
    let mut inventory = Inventory::new(&context, arguments.module.starts_with("std/"))?;
    let module = inventory.select(&arguments.module)?;
    let mut mir = inventory.solve_transform(&module)?;
    let sealed = mir.seal().map_err(|ds| ds.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    let ModuleTarget::Bound(root) = mir.roots[0] else { return Err("unresolved service entry".into()); };
    let symbol = *mir.exports[root.index()].iter().find(|s| mir.symbols[s.index()].name == "main")
        .ok_or("missing static service plan")?;
    let executable = sealed.seal_export(symbol).map_err(|ds| ds.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    let mut session = compile(&executable, inventory.runtime_options())?;
    let result = initialize(&mut session, &inventory, &mut mir.sources);
    diagnostics::finish(&session, &mir.sources, 0, result)?;
    let mut service = TransformSession::new(session)?;
    let names = crate::source_arg::service_source_names(&arguments.sources)?;
    if names != service.sources() {
        return Err(format!("service sources differ: declared {:?}, supplied {names:?}", service.sources()));
    }
    crate::source_arg::reject_stdin_sources(&arguments.sources)?;
    let limits = crate::execution_config_for(inventory.runtime_options())?.data_limits;
    let mut values = BTreeMap::new();
    for (name, input) in crate::source_arg::collect_service_sources(arguments.sources, limits.file_size)? {
        let id = mir.sources.try_add(input.source_name, &input.text).map_err(|e| e.to_string())?;
        let format = match input.format {
            telora_core::SystemDataFormat::Json => Format::Json,
            telora_core::SystemDataFormat::Yaml => Format::Yaml,
            telora_core::SystemDataFormat::Toml => Format::Toml,
        };
        let value = materialize(&mut service, &mir.sources, id, format, input.text.len())?;
        values.insert(name, value);
    }
    let result = service.initialize(&values);
    diagnostics::finish(service.session(), &mir.sources, 0, result)?;
    let request = mir.sources.try_add("@request", "").map_err(|e| e.to_string())?;
    let stdin = std::io::stdin();
    if !serve {
        let input = crate::source_arg::read_limited(stdin.lock(), limits.file_size, "query input")?;
        let input = String::from_utf8(input).map_err(|e| e.to_string())?;
        let response = transform(&mut service, &mut mir.sources, request, &input);
        for diagnostic in response["diagnostics"].as_array().into_iter().flatten() {
            crate::emit_stderr(serde_json::json!({"schema":"telora.execution/v1", "record":"diagnostic",
                "severity":diagnostic["severity"], "message":diagnostic["message"],
                "labels":diagnostic["labels"], "notes":diagnostic["notes"]}))?;
        }
        if response["error"] == true { return Ok(1); }
        crate::emit(response["ok"].clone())?;
        return Ok(0);
    }
    let mut reader = stdin.lock();
    loop {
        let mut bytes = Vec::new();
        let mut oversized = false;
        loop {
            let available = reader.fill_buf().map_err(|e| e.to_string())?;
            if available.is_empty() { break; }
            let end = available.iter().position(|b| *b == b'\n').map(|n| n + 1);
            let count = end.unwrap_or(available.len());
            if bytes.len().saturating_add(count) <= limits.file_size {
                bytes.extend_from_slice(&available[..count]);
            } else { oversized = true; }
            reader.consume(count);
            if end.is_some() { break; }
        }
        if bytes.is_empty() && !oversized { break; }
        let response = if oversized {
            failure("request exceeds file_size limit")
        } else { match std::str::from_utf8(&bytes) {
            Ok(input) => transform(&mut service, &mut mir.sources, request, input),
            Err(_) => failure("request is not UTF-8"),
        }};
        crate::emit(response)?;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    Ok(0)
}

fn materialize(service: &mut TransformSession, sources: &SourceDatabase, source: SourceId,
    format: Format, bytes: usize) -> Result<Value, String> {
    let plan = data_plan::parse_registered(sources, source, format)
        .map_err(|ds| ds.iter().map(|d| sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    data_plan::enforce_limits(&plan, crate::execution_config().data_limits, bytes)?;
    service.session_mut().register_data_sources(sources, &plan)?;
    service.session_mut().materialize_value(&plan)
}

fn transform(service: &mut TransformSession, sources: &mut SourceDatabase, request: SourceId, input: &str) -> serde_json::Value {
    let result = (|| {
        service.reset()?;
        sources.replace_unreferenced(request, "@request", input).map_err(|e| e.to_string())?;
        let input = materialize(service, sources, request, Format::Json, input.len())?;
        service.transform(input)
    })();
    match result {
        Ok(observed) => if let Some(ok) = observed.get("Ok") {
            serde_json::json!({"ok":ok[0],"error":false,"diagnostics":ok[1]})
        } else if let Some(errors) = observed.get("Err") {
            serde_json::json!({"ok":null,"error":true,"diagnostics":errors})
        } else { failure("invalid sealed transform result") },
        Err(error) => failure(&error),
    }
}

fn failure(message: &str) -> serde_json::Value {
    serde_json::json!({"ok":null,"error":true,"diagnostics":[{
        "severity":"Error", "message":message, "labels":[], "notes":[]
    }]})
}
