use super::*;

/// Host-facing types selected from the closed policy adapter signature.
/// No callback needs to discover its argument/result type in the VM.
#[derive(Clone, Copy, Debug)]
pub struct RunContract {
    pub env: TypeId,
    pub caps: TypeId,
    pub resources: TypeId,
    pub state: TypeId,
    pub event: TypeId,
    pub effects: TypeId,
}

pub struct RunCalls {
    pub contract: RunContract,
    pub(crate) unary: BytecodeFunction,
    pub(crate) binary: BytecodeFunction,
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
        unary: call_adapter(1),
        binary: call_adapter(2),
    });
    Ok(artifact)
}

fn run_contract(image: &crate::type_image::TypeImage, root: TypeId) -> Option<RunContract> {
    let parts = |id: TypeId, constructor: TypeConstructor, len: usize| {
        let ty = image.types.get(id.index())?;
        (ty.constructor == constructor && ty.arguments.len() == len)
            .then_some(ty.arguments.as_slice())
    };
    let config = parts(root, TypeConstructor::Function, 2)?;
    let configured = parts(config[1], TypeConstructor::Tuple, 2)?;
    let init = parts(configured[1], TypeConstructor::Function, 2)?;
    let initialized = parts(init[1], TypeConstructor::Tuple, 2)?;
    let reducer = parts(initialized[1], TypeConstructor::Function, 3)?;
    let transition = parts(reducer[2], TypeConstructor::Tuple, 2)?;
    parts(transition[1], TypeConstructor::Array, 1)?;
    if initialized[0] != reducer[0] || initialized[0] != transition[0] {
        return None;
    }
    let contract = RunContract {
        env: config[0],
        caps: configured[0],
        resources: init[0],
        state: initialized[0],
        event: reducer[1],
        effects: transition[1],
    };
    let mut pending = vec![
        contract.env,
        contract.caps,
        contract.resources,
        contract.state,
        contract.event,
        contract.effects,
    ];
    let mut visited = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let ty = image.types.get(id.index())?;
        if matches!(ty.constructor, TypeConstructor::Parameter(_)) {
            return None;
        }
        pending.extend(ty.arguments.iter().copied());
    }
    Some(contract)
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
