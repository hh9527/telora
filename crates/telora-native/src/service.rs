//! Native service state machine. All protocol types come from the sealed plan.
use crate::{abi::{CallContext, Result, TypeKey, Value}, jit::Compiled};
use telora_core::entry_plan::RunContract;

enum Phase {
    Configure(Value),
    Initialize { caps: Value, initializer: Value },
    Reduce { state: Value, reducer: Value },
    Failed,
}

pub struct ServiceSession {
    compiled: Compiled,
    context: CallContext,
    contract: RunContract,
    phase: Phase,
}

impl ServiceSession {
    /// Module initialization/publication must have succeeded before configuration.
    pub fn new(compiled: Compiled, context: CallContext, configure: Value, contract: RunContract) -> Result<Self> {
        if !context.runtime()?.is_published() || context.is_aborted() {
            return Err("native service requires successful module publication".into());
        }
        context.runtime()?.check_argument(&configure)?;
        Ok(Self { compiled, context, contract, phase: Phase::Configure(configure) })
    }

    pub fn context(&self) -> &CallContext { &self.context }
    pub fn context_mut(&mut self) -> &mut CallContext { &mut self.context }
    pub fn contract(&self) -> RunContract { self.contract }

    fn expect(value: &Value, ty: telora_core::mir::TypeId) -> Result<()> {
        if value.type_key() != TypeKey::try_from(ty)? {
            return Err("native service value differs from sealed protocol type".into());
        }
        Ok(())
    }

    fn pair(&self, value: &Value) -> Result<(Value, Value)> {
        let runtime = self.context.runtime()?;
        Ok((runtime.field(value, 0)?.to_owned(), runtime.field(value, 1)?.to_owned()))
    }

    pub fn configure(&mut self, env: Value) -> Result<Value> {
        if !matches!(self.phase, Phase::Configure(_)) { return Err("native service is not awaiting configuration".into()); }
        let Phase::Configure(configure) = std::mem::replace(&mut self.phase, Phase::Failed) else { unreachable!() };
        Self::expect(&env, self.contract.env)?;
        let result = self.compiled.call_closure(&mut self.context, &configure, &[env])?;
        let (caps, initializer) = self.pair(&result)?;
        Self::expect(&caps, self.contract.caps)?;
        self.phase = Phase::Initialize { caps: caps.clone(), initializer };
        Ok(caps)
    }

    pub fn capabilities(&self) -> Result<&Value> {
        match &self.phase {
            Phase::Initialize { caps, .. } => Ok(caps),
            _ => Err("native service is not awaiting resources".into()),
        }
    }

    pub fn initialize(&mut self, resources: Value) -> Result<()> {
        if !matches!(self.phase, Phase::Initialize { .. }) { return Err("native service is not awaiting resources".into()); }
        let Phase::Initialize { initializer, .. } = std::mem::replace(&mut self.phase, Phase::Failed) else { unreachable!() };
        Self::expect(&resources, self.contract.resources)?;
        let result = self.compiled.call_closure(&mut self.context, &initializer, &[resources])?;
        let (state, reducer) = self.pair(&result)?;
        Self::expect(&state, self.contract.state)?;
        self.phase = Phase::Reduce { state, reducer };
        Ok(())
    }

    pub fn reduce(&mut self, event: Value) -> Result<Value> {
        if !matches!(self.phase, Phase::Reduce { .. }) { return Err("native service is not ready for an event".into()); }
        let Phase::Reduce { state, reducer } = std::mem::replace(&mut self.phase, Phase::Failed) else { unreachable!() };
        Self::expect(&event, self.contract.event)?;
        let result = self.compiled.call_closure(&mut self.context, &reducer, &[state, event])?;
        let (state, effects) = self.pair(&result)?;
        Self::expect(&state, self.contract.state)?;
        Self::expect(&effects, self.contract.effects)?;
        self.phase = Phase::Reduce { state, reducer };
        Ok(effects)
    }

    /// Call after consuming the current event's effects. State/reducer are
    /// always roots; callers explicitly retain any additional descriptors.
    pub fn collect(&mut self, roots: &[Value]) -> Result<(Vec<Value>, crate::runtime::CollectionStats)> {
        let Phase::Reduce { state, reducer } = &self.phase else {
            return Err("native service collection requires an event boundary".into());
        };
        let all = [vec![state.clone(), reducer.clone()], roots.to_vec()].concat();
        let (mut relocated, stats) = self.context.collect_work(&all)?;
        let extra = relocated.split_off(2);
        let reducer = relocated.pop().unwrap();
        let state = relocated.pop().unwrap();
        self.phase = Phase::Reduce { state, reducer };
        Ok((extra, stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{runtime::Runtime, test_support};
    use telora_core::{entry_plan, mir::TypeState};

    #[test]
    fn service_keeps_state_and_quota_between_events_and_ends_after_failure() {
        let mir = test_support::graph(include_str!("../tests/fixtures/service-state.telora"));
        let symbol = *mir.exports.iter().flatten().find(|s| mir.symbols[s.index()].name == "configure").unwrap();
        let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else { panic!("closed configure") };
        let sealed = mir.seal().unwrap();
        let contract = entry_plan::run_contract(sealed.types(), ty).unwrap();
        let executable = sealed.seal_export(symbol).unwrap();
        let compiled = crate::jit::compile_executable(&executable).unwrap();
        let mut context = CallContext::with_runtime(Runtime::new(executable.sealed_mir()).unwrap()).with_fuel(10000);
        compiled.initialize(&mut context).unwrap();
        let configure = compiled.export(&mut context, symbol).unwrap();
        let mut service = ServiceSession::new(compiled, context, configure, contract).unwrap();
        let int = TypeKey::try_from(contract.env).unwrap();
        let scalar = |service: &ServiceSession, value| service.context().runtime().unwrap().scalar(int, [0; 3], value).unwrap();
        assert!(service.reduce(scalar(&service, 0)).is_err());
        service.configure(scalar(&service, 10)).unwrap();
        assert!(service.configure(scalar(&service, 0)).is_err());
        service.initialize(scalar(&service, 2)).unwrap();
        let mut fuel = service.context().remaining_fuel().unwrap();
        for expected in [13, 14, 15] {
            let effects = service.reduce(scalar(&service, 1)).unwrap();
            let rt = service.context().runtime().unwrap();
            assert_eq!(rt.scalar_bits(rt.array_get(&effects, 0).unwrap()).unwrap(), expected);
            assert!(service.context().remaining_fuel().unwrap() < fuel);
            fuel = service.context().remaining_fuel().unwrap();
            service.collect(&[]).unwrap();
            assert!(service.context().runtime().unwrap().array_len(&effects).is_err());
            assert_eq!(service.context().remaining_fuel().unwrap(), fuel);
        }
        assert!(service.reduce(scalar(&service, 0)).is_err());
        let fuel = service.context().remaining_fuel();
        assert!(service.reduce(scalar(&service, 1)).is_err());
        assert_eq!(service.context().remaining_fuel(), fuel);
        assert_eq!(service.context().diagnostics().len(), 1);
        assert_eq!(service.context().diagnostics()[0].message, "service event failed");
    }
}
