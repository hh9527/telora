use super::*;

pub use crate::entry_plan::{RunMode, RunContract};
use crate::entry_plan::run_contract;

pub struct RunCalls {
    pub contract: RunContract,
    pub(crate) protocol: Option<RunHostTypes>,
    pub(crate) unary: BytecodeFunction,
    pub(crate) binary: BytecodeFunction,
    pub(crate) resources: BytecodeFunction,
}

#[derive(Clone, Copy)]
pub(crate) struct RunHostTypes {
    pub value: TypeId,
    pub mode: TypeId,
    pub format: TypeId,
}

fn host_types(image: &crate::type_image::TypeImage, contract: RunContract) -> Option<RunHostTypes> {
    let field = |id: TypeId, name: &str| {
        let TypeConstructor::Nominal(symbol) = image.types.get(id.index())?.constructor else {
            return None;
        };
        image
            .definition(symbol)?
            .members
            .iter()
            .find(|m| m.name == name)?
            .payload
    };
    let data = field(contract.resources, "data")?;
    let item = *image.types.get(data.index())?.arguments.first()?;
    let value = *image.types.get(item.index())?.arguments.first()?;
    let sources = field(contract.env, "sources")?;
    let source = *image.types.get(sources.index())?.arguments.first()?;
    Some(RunHostTypes {
        value,
        mode: field(contract.env, "mode")?,
        format: field(source, "fmt")?,
    })
}

/// The compiler-owned source adapter has shape
/// Env -> (Caps, Resources -> (State, (State, Event) -> (State, Effects))).
/// It captures the application and policy in their one shared execution graph.
pub fn compile_run(
    sealed: SealedMir<'_>,
    entry: SymbolId,
) -> Result<CompiledEntry, Vec<Diagnostic>> {
    let location = sealed
        .mir()
        .symbols
        .get(entry.index())
        .and_then(|symbol| symbol.declarations.first())
        .map(|node| sealed.mir().hir[node.index()].location);
    let mut artifact = compile(sealed, entry)?;
    let contract = run_contract(&artifact.types, artifact.result_type).ok_or_else(|| {
        vec![Diagnostic::error(
            "run policy adapter must have a closed config/initializer/reducer signature with one state type",
            location.expect("compiled entry has a declaration"),
        )]
    })?;
    artifact.run_calls = Some(RunCalls {
        contract,
        protocol: host_types(&artifact.types, contract),
        unary: call_adapter(1),
        binary: call_adapter(2),
        resources: call_adapter(3),
    });
    Ok(artifact)
}


fn call_adapter(arity: usize) -> BytecodeFunction {
    use crate::bytecode::{Instruction as I, Register};
    BytecodeFunction::with_signature(
        format!("<solved entry call/{arity}>"),
        arity + 1,
        0,
        arity + 1,
        vec![],
        vec![
            I::Call {
                base: Register(0),
                argument_count: arity,
            },
            I::Return { src: Register(0) },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_policy_contracts_before_creating_a_vm() {
        for source in [
            "export def answer = 42;",
            "export def answer = fn(env: Int) { (0, fn(resources: Int) { (0, fn(state: String, event: Int) { (state, [0]) }) }) };",
            "export def answer = fn(env: Int) { (0, fn(resources: Int) { (0, fn(state: Int, event: Int) { (state, 0) }) }) };",
        ] {
            let mir = crate::codegen::tests::graph(source, "");
            let result = compile_run(mir.seal().unwrap(), crate::codegen::tests::entry(&mir));
            assert!(result.is_err(), "{source}");
            assert!(result.err().unwrap()[0].message.contains("policy adapter"));
        }
    }
}
