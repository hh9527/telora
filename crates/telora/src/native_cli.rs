//! Experimental native session. The default execution path is independent.
use crate::static_input::Inventory;
use telora_core::{Diagnostic, SourceDatabase, mir::SealedMir, source::Severity};
use telora_native::{
    abi::CallContext,
    jit::{self, Compiled},
    runtime::Runtime,
};

pub(crate) struct Session {
    pub compiled: Compiled,
    pub context: CallContext,
}

impl Session {
    pub fn compile(sealed: &SealedMir<'_>) -> Result<Self, String> {
        let graph = sealed.mir();
        let modules = graph
            .hir
            .iter()
            .map(|node| node.module)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let compiled = jit::compile_modules(sealed, &modules, &[])?;
        let context = CallContext::with_runtime(Runtime::new(sealed)?)
            .with_fuel(crate::execution_config().session_quota.fuel as u64);
        Ok(Self { compiled, context })
    }

    pub fn initialize(
        &mut self,
        inventory: &Inventory,
        sources: &mut SourceDatabase,
    ) -> Vec<Diagnostic> {
        let data_result = (|| -> Result<(), Vec<Diagnostic>> {
            for module in self.compiled.data_modules() {
                let (format, text) = inventory
                    .read_data_text(
                        &module.name,
                        crate::execution_config().data_limits.file_size,
                    )
                    .map_err(|e| vec![error(e)])?;
                let source = sources
                    .try_add(module.name.clone(), &text)
                    .map_err(|e| vec![error(e.to_string())])?;
                let plan = telora_core::data_plan::parse_registered(sources, source, format)?;
                telora_core::data_plan::enforce_limits(&plan, crate::execution_config().data_limits, text.len())
                    .map_err(|message| vec![Diagnostic::error(message, telora_core::Loc { source, start: 0, end: 0 })])?;
                self.compiled
                    .inject_data(&mut self.context, module.symbol, &plan)
                    .map_err(|e| vec![error(e)])?;
            }
            Ok(())
        })();
        if let Err(diagnostics) = data_result {
            return diagnostics;
        }
        let result = self.compiled.initialize(&mut self.context);
        let mut diagnostics = self.diagnostics(sources);
        if let Err(message) = result {
            if !diagnostics.iter().any(|d| d.severity == Severity::Error) {
                diagnostics.push(error(message));
            }
        }
        diagnostics
    }

    fn diagnostics(&self, sources: &SourceDatabase) -> Vec<Diagnostic> {
        self.context
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let [source, start, end] = diagnostic.origin.words();
                let mut result = match sources.files().find(|file| file.id().get() == source) {
                    Some(file) => Diagnostic::error(
                        &diagnostic.message,
                        telora_core::Loc {
                            source: file.id(),
                            start,
                            end,
                        },
                    ),
                    None => error(&diagnostic.message),
                };
                result.severity = diagnostic.severity;
                for (index, subject) in diagnostic.subjects.iter().enumerate() {
                    if *subject == diagnostic.origin {
                        continue;
                    }
                    let [source, start, end] = subject.words();
                    if let Some(file) = sources.files().find(|file| file.id().get() == source) {
                        result = result.with_secondary(
                            format!("subject {} originated here", index + 1),
                            telora_core::Loc {
                                source: file.id(),
                                start,
                                end,
                            },
                        );
                    }
                }
                result
            })
            .collect()
    }
}

pub(crate) fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message: message.into(),
        labels: vec![],
        notes: vec![],
    }
}

pub(crate) fn eval(context: std::path::PathBuf, module: &str, export: &str) -> Result<i32, String> {
    use telora_core::mir::{ModuleTarget, ResolveState, TypeState};
    use telora_native::{abi::TypeKey, runtime::DataContract};
    let mut inventory = Inventory::new(&context, module.starts_with("std/"))?;
    let root = inventory.select(module)?;
    let mut mir = inventory.solve(&root);
    let sealed = mir.seal().map_err(|diagnostics| {
        mir.diagnostics
            .iter()
            .chain(&diagnostics)
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let ModuleTarget::Bound(module) = mir.roots[0] else {
        return Err("unresolved native eval module".into());
    };
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|s| mir.symbols[s.index()].name == export)
        .ok_or_else(|| format!("module has no export {export:?}"))?;
    let ResolveState::Bound(target) = mir.symbols[symbol.index()].resolution else {
        return Err("unresolved native eval export".into());
    };
    let contract = DataContract::from_mir(&sealed)?;
    if !mir.symbol_generics[target.index()].is_empty()
        || !matches!(mir.ty_slots[mir.symbol_types[target.index()].index()], TypeState::Known(ty) if TypeKey::try_from(ty)? == contract.value_type())
    {
        return Err("eval export must have the authoritative std/value.Value type".into());
    }
    let mut session = Session::compile(&sealed)?;
    let diagnostics = session.initialize(&inventory, &mut mir.sources);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    for diagnostic in &diagnostics { eprintln!("{}", mir.sources.render(diagnostic)); }
    let value = session.compiled.export(&mut session.context, symbol)?;
    let text = session
        .context
        .runtime()?
        .semantic_json(&contract, &value)?;
    println!("{text}");
    Ok(0)
}

pub(crate) fn eval_with(
    directory: std::path::PathBuf,
    module_name: &str,
    export: &str,
    inputs: Vec<crate::source_arg::NamedSource>,
    args: Vec<String>,
) -> Result<i32, String> {
    use std::collections::BTreeMap;
    use telora_core::mir::{ModuleTarget, ResolveState, TypeConstructor as T, TypeState};
    use telora_native::{abi::TypeKey, runtime::DataContract};
    let mut inventory = Inventory::new(&directory, module_name.starts_with("std/"))?;
    let root = inventory.select(module_name)?;
    let mut mir = inventory.solve(&root);
    let sealed = mir.seal().map_err(|diagnostics| {
        mir.diagnostics
            .iter()
            .chain(&diagnostics)
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let ModuleTarget::Bound(module) = mir.roots[0] else {
        return Err("unresolved eval-with module".into());
    };
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|s| mir.symbols[s.index()].name == export)
        .ok_or_else(|| format!("module has no export {export:?}"))?;
    let ResolveState::Bound(target) = mir.symbols[symbol.index()].resolution else {
        return Err("unresolved eval-with export".into());
    };
    let expected = mir
        .modules
        .iter()
        .position(|m| m.name == "std/entry")
        .and_then(|module| {
            mir.exports[module]
                .iter()
                .find(|s| mir.symbols[s.index()].name == "Eval")
        })
        .and_then(
            |symbol| match mir.ty_slots[mir.symbol_types[symbol.index()].index()] {
                TypeState::Known(meta) if mir.types[meta.index()].constructor == T::Meta => {
                    mir.types[meta.index()].arguments.first().copied()
                }
                _ => None,
            },
        )
        .ok_or("eval-with export: expected Eval (std/entry.Eval)")?;
    if !mir.symbol_generics[target.index()].is_empty()
        || mir.ty_slots[mir.symbol_types[target.index()].index()] != TypeState::Known(expected)
    {
        return Err("eval-with export: expected Eval (std/entry.Eval)".into());
    }
    // Resolve every host-facing field/type once from the sealed skeleton.
    let fields = |ty: telora_core::mir::TypeId| -> Result<BTreeMap<String, (usize, telora_core::mir::TypeId)>, String> {
        let body = sealed.types().layout(ty).map_or(ty, |layout| layout.body);
        let shape = &mir.types[body.index()];
        let T::Record(names) = &shape.constructor else { return Err("entry contract requires a record".into()); };
        Ok(names.iter().cloned().zip(shape.arguments.iter().copied()).enumerate().map(|(i, (name, ty))| (name, (i, ty))).collect())
    };
    let entry_fields = fields(expected)?;
    let &(config_index, config_type) = entry_fields.get("config").ok_or("Eval.config missing")?;
    let &(evaluate_index, evaluate_type) = entry_fields
        .get("evaluate")
        .ok_or("Eval.evaluate missing")?;
    let signature = &mir.types[evaluate_type.index()];
    let contract = DataContract::from_mir(&sealed)?;
    if signature.constructor != T::Function
        || signature.arguments.len() != 2
        || TypeKey::try_from(signature.arguments[1])? != contract.value_type()
    {
        return Err("Eval.evaluate has invalid sealed signature".into());
    }
    let context_type = signature.arguments[0];
    let config_fields = fields(config_type)?;
    let context_fields = fields(context_type)?;
    let &(args_index, _) = config_fields.get("args").ok_or("config.args missing")?;
    let &(sources_index, _) = config_fields
        .get("sources")
        .ok_or("config.sources missing")?;
    let &(envs_index, _) = config_fields.get("envs").ok_or("config.envs missing")?;
    let args_type = context_fields.get("args").ok_or("Context.args missing")?.1;
    let env_type = context_fields.get("env").ok_or("Context.env missing")?.1;
    let sources_type = context_fields
        .get("sources")
        .ok_or("Context.sources missing")?
        .1;
    let string_type = *mir.types[args_type.index()]
        .arguments
        .first()
        .ok_or("Context.args element missing")?;
    let (context_type, args_type, env_type, sources_type, string_type) = (
        TypeKey::try_from(context_type)?,
        TypeKey::try_from(args_type)?,
        TypeKey::try_from(env_type)?,
        TypeKey::try_from(sources_type)?,
        TypeKey::try_from(string_type)?,
    );
    let mut session = Session::compile(&sealed)?;
    let diagnostics = session.initialize(&inventory, &mut mir.sources);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    for diagnostic in &diagnostics { eprintln!("{}", mir.sources.render(diagnostic)); }
    let initial_diagnostic_count = session.context.diagnostics().len();
    let entry = session.compiled.export(&mut session.context, symbol)?;
    let rt = session.context.runtime()?;
    let config = rt.field(&entry, config_index)?.to_owned();
    let evaluate = rt.field(&entry, evaluate_index)?.to_owned();
    let names = |index, field: &str| -> Result<Vec<String>, String> {
        let array = rt.field(&config, index)?.to_owned();
        let mut names = (0..rt.array_len(&array)?)
            .map(|i| Ok(rt.text(rt.array_get(&array, i)?)?.as_str().to_owned()))
            .collect::<Result<Vec<_>, String>>()?;
        names.sort();
        if names.iter().any(String::is_empty) || names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(format!(
                "entry.Eval.config.{field} must contain unique non-empty names"
            ));
        }
        Ok(names)
    };
    let declared_sources = names(sources_index, "sources")?;
    let declared_envs = names(envs_index, "envs")?;
    let provided = crate::source_arg::eval_source_names(&inputs)?;
    if declared_sources != provided {
        return Err(format!(
            "eval sources do not match entry.Eval config: declared {declared_sources:?}, provided {provided:?}"
        ));
    }
    if rt.scalar_bits(rt.field(&config, args_index)?)? == 0 && !args.is_empty() {
        return Err("entry.Eval config does not accept command-line arguments".into());
    }
    let env = declared_envs
        .into_iter()
        .map(|name| {
            std::env::var(&name)
                .map(|text| (name.clone(), text))
                .map_err(|_| format!("cannot read declared environment variable {name:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let inputs = crate::source_arg::collect_eval_sources(
        inputs,
        crate::execution_config().data_limits.file_size,
    )?;
    let rt = session.context.runtime_mut()?;
    let mut source_values = Vec::new();
    for (name, source) in inputs {
        let id = mir
            .sources
            .try_add(source.source_name, &source.text)
            .map_err(|e| e.to_string())?;
        let format = match source.format {
            telora_core::SystemDataFormat::Json => telora_core::data_plan::Format::Json,
            telora_core::SystemDataFormat::Yaml => telora_core::data_plan::Format::Yaml,
            telora_core::SystemDataFormat::Toml => telora_core::data_plan::Format::Toml,
        };
        let plan =
            telora_core::data_plan::parse_registered(&mir.sources, id, format).map_err(|ds| {
                ds.iter()
                    .map(|d| mir.sources.render(d))
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
        telora_core::data_plan::enforce_limits(&plan, crate::execution_config().data_limits, source.text.len())
            .map_err(|message| mir.sources.render(&Diagnostic::error(message, telora_core::Loc { source: id, start: 0, end: 0 })))?;
        let value = rt.materialize_data(&contract, &plan)?;
        source_values.push((rt.string(string_type, [0; 3], &name)?, value));
    }
    let sources = rt.dict(sources_type, [0; 3], &source_values)?;
    let mut values = Vec::new();
    for (name, text) in env {
        values.push((
            rt.string(string_type, [0; 3], &name)?,
            rt.string(string_type, [0; 3], &text)?,
        ));
    }
    let env = rt.dict(env_type, [0; 3], &values)?;
    let values = args
        .iter()
        .map(|text| rt.string(string_type, [0; 3], text))
        .collect::<Result<Vec<_>, _>>()?;
    let args = rt.array(args_type, [0; 3], &values)?;
    let mut values = BTreeMap::from([("args", args), ("env", env), ("sources", sources)]);
    let fields = context_fields
        .keys()
        .map(|name| {
            values
                .remove(name.as_str())
                .ok_or("unexpected Context field".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let argument = rt.aggregate(context_type, [0; 3], &fields)?;
    let value = session
        .compiled
        .call_closure(&mut session.context, &evaluate, &[argument])
        .map_err(|e| {
            if !session.context.diagnostics().iter().any(|d| d.severity == Severity::Error) {
                e
            } else {
                session
                    .diagnostics(&mir.sources)
                    .iter()
                    .skip(initial_diagnostic_count)
                    .map(|d| mir.sources.render(d))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        })?;
    for diagnostic in session.diagnostics(&mir.sources).iter().skip(initial_diagnostic_count) { eprintln!("{}", mir.sources.render(diagnostic)); }
    println!(
        "{}",
        session
            .context
            .runtime()?
            .semantic_json(&contract, &value)?
    );
    Ok(0)
}
