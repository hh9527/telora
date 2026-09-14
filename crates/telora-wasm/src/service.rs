//! Service phases retain typed Wasm handles, never deserialize state or closures.
use crate::{session::Session, transport::Value};
use telora_core::entry_plan::RunContract;

enum Phase {
    Configure(Value),
    Initialize { caps: Value, initializer: Value },
    Reduce { state: Value, reducer: Value },
    Failed,
}

pub struct ServiceSession {
    session: Session,
    contract: RunContract,
    phase: Phase,
}

impl ServiceSession {
    /// The module graph must be initialized before constructing its service.
    pub fn new(mut session: Session, contract: RunContract) -> Result<Self, String> {
        let pointer = session.entry()?;
        let configure = Value {
            pointer,
            ty: session.manifest.entry_type,
        };
        Ok(Self {
            session,
            contract,
            phase: Phase::Configure(configure),
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }
    pub fn contract(&self) -> RunContract {
        self.contract
    }

    pub fn configure(&mut self, env: Value) -> Result<Value, String> {
        if !matches!(self.phase, Phase::Configure(_)) {
            return Err("Wasm service is not awaiting configuration".into());
        }
        let Phase::Configure(configure) = std::mem::replace(&mut self.phase, Phase::Failed) else {
            unreachable!()
        };
        self.session
            .expect_value(env, self.contract.env.index() as u32)?;
        let result = self.session.invoke_values(configure, &[env])?;
        let (caps, initializer) = self.session.pair(result)?;
        self.session
            .expect_value(caps, self.contract.caps.index() as u32)?;
        self.phase = Phase::Initialize { caps, initializer };
        Ok(caps)
    }

    pub fn capabilities(&self) -> Result<Value, String> {
        match self.phase {
            Phase::Initialize { caps, .. } => Ok(caps),
            _ => Err("Wasm service is not awaiting resources".into()),
        }
    }

    pub fn initialize(&mut self, resources: Value) -> Result<(), String> {
        if !matches!(self.phase, Phase::Initialize { .. }) {
            return Err("Wasm service is not awaiting resources".into());
        }
        let Phase::Initialize { initializer, .. } =
            std::mem::replace(&mut self.phase, Phase::Failed)
        else {
            unreachable!()
        };
        self.session
            .expect_value(resources, self.contract.resources.index() as u32)?;
        let result = self.session.invoke_values(initializer, &[resources])?;
        let (state, reducer) = self.session.pair(result)?;
        self.session
            .expect_value(state, self.contract.state.index() as u32)?;
        self.phase = Phase::Reduce { state, reducer };
        Ok(())
    }

    pub fn reduce(&mut self, event: Value) -> Result<Value, String> {
        if !matches!(self.phase, Phase::Reduce { .. }) {
            return Err("Wasm service is not ready for an event".into());
        }
        let Phase::Reduce { state, reducer } = std::mem::replace(&mut self.phase, Phase::Failed)
        else {
            unreachable!()
        };
        self.session
            .expect_value(event, self.contract.event.index() as u32)?;
        let result = self.session.invoke_values(reducer, &[state, event])?;
        let (state, effects) = self.session.pair(result)?;
        self.session
            .expect_value(state, self.contract.state.index() as u32)?;
        self.session
            .expect_value(effects, self.contract.effects.index() as u32)?;
        self.phase = Phase::Reduce { state, reducer };
        Ok(effects)
    }
}
