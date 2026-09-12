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
        let context = CallContext::with_runtime(Runtime::new(sealed)?);
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
        let mut diagnostics = self
            .context
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let [source, start, end] = diagnostic.origin.words();
                match sources.files().find(|file| file.id().get() == source) {
                    Some(file) => Diagnostic::error(
                        &diagnostic.message,
                        telora_core::Loc {
                            source: file.id(),
                            start,
                            end,
                        },
                    ),
                    None => error(&diagnostic.message),
                }
            })
            .collect::<Vec<_>>();
        if let Err(message) = result {
            if diagnostics.is_empty() {
                diagnostics.push(error(message));
            }
        }
        diagnostics
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
    if !diagnostics.is_empty() {
        return Err(diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let value = session.compiled.export(&mut session.context, symbol)?;
    let text = session
        .context
        .runtime()?
        .semantic_json(&contract, &value)?;
    println!("{text}");
    Ok(0)
}
