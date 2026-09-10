//! Link already-compiled ABI references. No source parsing, resolution, type
//! inference or Telora execution happens here.
use crate::{
    NativeFunction,
    bytecode::{BytecodeFunction, Constant},
    codegen::{CompiledEntry, NativeLink},
    source::Diagnostic,
};
use std::collections::BTreeMap;

/// Executable and its sealed static type data, ready to move into a VM session.
pub struct LinkedEntry {
    pub(crate) bytecode: BytecodeFunction,
    pub(crate) types: crate::type_image::TypeImage,
    pub(crate) result_type: crate::mir::TypeId,
    pub(crate) eval_call: Option<crate::codegen::EvalCall>,
}

pub fn link_entry(artifact: CompiledEntry) -> Result<LinkedEntry, Vec<Diagnostic>> {
    let bytecode = link_builtins(&artifact)?;
    Ok(LinkedEntry {
        bytecode,
        types: artifact.types,
        result_type: artifact.result_type,
        eval_call: artifact.eval_call,
    })
}

/// The result type comes from static solving, never from inspecting the value.
pub struct SolvedExecution {
    pub(crate) world: crate::ExecutionWorld,
    pub(crate) result_type: crate::mir::TypeId,
}

impl SolvedExecution {
    pub fn to_json(&self, value_type: crate::mir::TypeId) -> Result<String, String> {
        if self.result_type != value_type {
            return Err("eval result must be std/value.Value".into());
        }
        self.world.solved_json(value_type)
    }
    pub fn value(&self) -> crate::ValueRef<'_> {
        self.world.value()
    }
    pub fn result_type(&self) -> crate::mir::TypeId {
        self.result_type
    }
    pub fn types(&self) -> &crate::type_image::TypeImage {
        self.world
            .solved_types()
            .expect("solved execution owns its type image")
    }
}

pub fn link_with(
    artifact: &CompiledEntry,
    mut native: impl FnMut(&NativeLink) -> Option<NativeFunction>,
) -> Result<BytecodeFunction, Vec<Diagnostic>> {
    let mut replacements = BTreeMap::new();
    let mut diagnostics = vec![];
    for link in &artifact.native_links {
        match native(link) {
            Some(function) if function.arity() == link.arity => {
                replacements.insert(link.constant, Constant::Native(function));
            }
            Some(_) => diagnostics.push(Diagnostic::error(
                "native ABI arity does not match the solved signature",
                link.location,
            )),
            None => diagnostics.push(Diagnostic::error(
                format!(
                    "native ABI binding is unavailable: {:?}/{}",
                    link.module, link.name
                ),
                link.location,
            )),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let mut index = 0;
    Ok(artifact.bytecode.relink_with(
        |constant| {
            let value = replacements
                .remove(&index)
                .unwrap_or_else(|| constant.clone());
            index += 1;
            value
        },
        |text| text.into(),
        std::sync::Arc::clone,
    ))
}

/// Admission uses the trusted module ABI identity. Source aliases have already
/// resolved to the defining symbol; export spelling is only an ABI linker key.
pub fn link_builtins(artifact: &CompiledEntry) -> Result<BytecodeFunction, Vec<Diagnostic>> {
    let registry = crate::core::module_specs()
        .into_iter()
        .flat_map(|module| {
            module
                .functions
                .into_iter()
                .map(move |(name, function)| ((module.native_id, name), function))
        })
        .collect::<BTreeMap<_, _>>();
    link_with(artifact, |link| {
        registry.get(&(link.module?, link.name.as_str())).copied()
    })
}
