//! Static service entry plan, shared by execution backends before code generation.
use crate::mir::{TypeId, TypeConstructor};

#[derive(Clone, Copy, Debug)]
pub enum RunMode {
    Run,
    Serve,
}

impl RunMode {
    pub fn policy_source(self) -> &'static str {
        match self {
            Self::Run => include_str!("../modules/std/_entry/run.telora"),
            Self::Serve => include_str!("../modules/std/_entry/serve.telora"),
        }
    }

    pub fn policy_module(self) -> &'static str {
        match self {
            Self::Run => "std/_entry/run",
            Self::Serve => "std/_entry/serve",
        }
    }

    /// This source is added before module/symbol/type solving. It introduces no
    /// runtime type checks and has no permission to resolve additional imports.
    pub fn adapter_source(self, module: &str, export: &str) -> Result<String, String> {
        let mut chars = export.chars();
        if !chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
            || !chars.all(|c| c == '_' || c.is_alphanumeric())
        {
            return Err("entry export must be an identifier".into());
        }
        let module = serde_json::to_string(module).map_err(|e| e.to_string())?;
        let policy = self.policy_module();
        let family = match self {
            Self::Run => "Run",
            Self::Serve => "Serve",
        };
        Ok(format!(
            r#"
            import {module} {{ {export} as selected }};
            import "{policy}" as policy;
            import "std/entry" as entry;
            import "std/_rt" as rt;
            def adapt: for(State) Fn(entry.{family}(State)) -> policy.MainType = fn(app) {{
                {{ config: app.config, ees: app.ees, start: app.start }}
            }};
            def main = adapt(selected);
            export def configure = fn(env: rt.Env) {{
                let configured = policy.config(env, main);
                (configured.0, fn(resources: rt.SystemResources) {{ configured.1(resources, main) }})
            }};
        "#
        ))
    }
}

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

pub fn run_contract(image: &crate::type_image::TypeImage, root: TypeId) -> Option<RunContract> {
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
