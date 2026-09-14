use super::*;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};
use telora_core::{
    RunHost, SourceDatabase, SystemEvent,
    entry_plan::{self, RunMode},
    mir::{Mir, SymbolId, TypeState},
};
use telora_wasm::{service::ServiceSession, service_output::ServiceEffect, transport::Value};

pub(crate) async fn execute(
    mut mir: Mir,
    inventory: Inventory,
    symbol: SymbolId,
    mode: RunMode,
    arguments: crate::ApplicationArgs,
    inputs: crate::source_arg::CollectedEntrySources,
) -> Result<i32, String> {
    let sealed = mir.seal().map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else {
        return Err("unclosed service signature".into());
    };
    let contract = entry_plan::run_contract(sealed.types(), ty)
        .ok_or("service requires a closed policy signature")?;
    let executable = sealed.seal_export(symbol).map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let mut session = compile(&executable)?;
    let result = initialize(&mut session, &inventory, &mut mir.sources);
    diagnostics::finish(&session, &mir.sources, 0, result)?;
    let mut service = ServiceSession::new(session, contract)?;
    let mut host = crate::ProcessRunHost::new(inputs.locators, arguments.ees_vars);
    let result = execute_inner(
        &mut service,
        &mut mir.sources,
        mode,
        &arguments.args,
        &inputs.entry,
        &mut host,
    )
    .await;
    let finished = host.finish().await;
    let (output, code) = match (result, finished) {
        (Ok(result), Ok(())) => result,
        (_, Err(error)) | (Err(error), Ok(())) => return Err(error),
    };
    std::io::stdout()
        .write_all(output.as_bytes())
        .and_then(|()| std::io::stdout().flush())
        .map_err(|e| format!("cannot write Entry output: {e}"))?;
    i32::try_from(code).map_err(|_| format!("Entry exit status {code} is outside the Host range"))
}

fn input(
    service: &mut ServiceSession,
    sources: &mut SourceDatabase,
    name: String,
    format: telora_core::SystemDataFormat,
    text: &str,
) -> Result<Value, String> {
    let source = sources.try_add(name, text).map_err(|e| e.to_string())?;
    let format = match format {
        telora_core::SystemDataFormat::Json => telora_core::data_plan::Format::Json,
        telora_core::SystemDataFormat::Yaml => telora_core::data_plan::Format::Yaml,
        telora_core::SystemDataFormat::Toml => telora_core::data_plan::Format::Toml,
    };
    let plan = telora_core::data_plan::parse_registered(sources, source, format).map_err(|ds| {
        ds.iter()
            .map(|d| sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    telora_core::data_plan::enforce_limits(
        &plan,
        crate::execution_config().data_limits,
        text.len(),
    )
    .map_err(|message| {
        sources.render(&Diagnostic::error(
            message,
            telora_core::Loc {
                source,
                start: 0,
                end: 0,
            },
        ))
    })?;
    service
        .session_mut()
        .register_data_sources(sources, &plan)?;
    service.session_mut().materialize_value(&plan)
}

async fn execute_inner(
    service: &mut ServiceSession,
    sources: &mut SourceDatabase,
    mode: RunMode,
    args: &[String],
    inputs: &telora_core::EntryDataSources,
    host: &mut crate::ProcessRunHost,
) -> Result<(String, i64), String> {
    let timer = PhaseTimer::new("service_setup");
    let contract = service.contract();
    let env =
        service
            .session_mut()
            .service_env(contract, mode, args, inputs, &host.ees_actors())?;
    let before = service.session().diagnostics()?.len();
    let result = service.configure(env);
    let caps_value = diagnostics::finish(service.session(), sources, before, result)?;
    let caps = service.session().service_caps(caps_value)?;
    host.configure(caps.clone())
        .await
        .map_err(|e| format!("cannot satisfy Entry capabilities: {e}"))?;
    let mut prepared = BTreeMap::new();
    for (name, request) in &caps.data_sources {
        if let Some(text) = host
            .read_data_source(request, crate::execution_config().data_limits.file_size)
            .await?
        {
            prepared.insert(
                name.clone(),
                input(service, sources, request.src.clone(), request.format, &text)?,
            );
        }
    }
    let mut texts = BTreeMap::new();
    for (name, request) in &caps.text_sources {
        let text =
            match std::fs::read_to_string(&request.src) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => request
                    .default
                    .clone()
                    .ok_or_else(|| format!("cannot read text source {:?}: {error}", request.src))?,
                Err(error) => {
                    return Err(format!(
                        "cannot read text source {:?}: {error}",
                        request.src
                    ));
                }
            };
        texts.insert(name.clone(), text);
    }
    let mut vars = BTreeMap::new();
    for name in &caps.vars {
        match std::env::var(name) {
            Ok(value) => {
                vars.insert(name.clone(), value);
            }
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(format!("cannot read variable {name:?}: {error}")),
        }
    }
    let stdin = if caps.stdin == telora_core::SystemStdin::Text {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("cannot read standard input: {e}"))?;
        Some(text)
    } else {
        None
    };
    let resources = service.session_mut().service_resources(
        contract,
        caps_value,
        prepared,
        &texts,
        &vars,
        stdin.as_deref(),
    )?;
    let before = service.session().diagnostics()?.len();
    let result = service.initialize(resources);
    diagnostics::finish(service.session(), sources, before, result)?;
    drop(timer);
    let mut next = None;
    let mut output = String::new();
    loop {
        let reply = if let Some(SystemEvent::EesReply(reply)) = &next {
            match &reply.result {
                Ok(value) => Some(input(
                    service,
                    sources,
                    "<EES reply>".into(),
                    telora_core::SystemDataFormat::Json,
                    &value.to_string(),
                )?),
                Err(_) => None,
            }
        } else {
            None
        };
        let event = service.session_mut().service_event(contract, next, reply)?;
        let before = service.session().diagnostics()?.len();
        let timer = PhaseTimer::new("service_reduce");
        let result = service.reduce(event);
        drop(timer);
        let effects = diagnostics::finish(service.session(), sources, before, result)?;
        // Validate the whole transition before performing any external effect.
        let effects = service.session().service_effects(effects, &caps)?;
        for effect in effects {
            match effect {
                ServiceEffect::Output(text) => output.push_str(&text),
                ServiceEffect::EesCall(call) => host.ees_call(call).await?,
                ServiceEffect::Exit(code) => return Ok((output, code)),
            }
        }
        let timer = PhaseTimer::new("service_collect");
        service.collect(&[])?;
        drop(timer);
        next = Some(
            host.next_event()
                .await?
                .ok_or("Entry made no progress and the Host has no pending event")?,
        );
    }
}
