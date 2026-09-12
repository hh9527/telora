use super::*;
use telora_core::mir::{PropertySite, SymbolId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DemandKey {
    Export(SymbolId),
    Property {
        owner: TypeId,
        site: PropertySite,
        property: TypeId,
    },
}
enum State {
    Pending,
    Evaluating,
    Ready(Value),
    Failed,
}
pub(super) struct DemandSlot {
    ty: TypeId,
    state: State,
}
#[derive(Debug)]
pub enum Demand {
    Evaluate,
    Ready(Value),
    Failed,
}

impl Runtime {
    pub fn register_demand(&mut self, key: DemandKey, ty: TypeId) -> Result<()> {
        if self.published {
            return Err("main world is already sealed".into());
        }
        self.layout(ty)?;
        if self.demands.contains_key(&key) {
            return Err("native demand already registered".into());
        }
        self.demands.insert(
            key,
            DemandSlot {
                ty,
                state: State::Pending,
            },
        );
        Ok(())
    }
    pub fn begin_demand(&mut self, key: DemandKey) -> Result<Demand> {
        let slot = self
            .demands
            .get_mut(&key)
            .ok_or("native demand missing from code plan")?;
        match &slot.state {
            State::Pending => {
                slot.state = State::Evaluating;
                Ok(Demand::Evaluate)
            }
            State::Evaluating => {
                slot.state = State::Failed;
                Err(format!("native initialization dependency cycle at {key:?}"))
            }
            State::Ready(value) => Ok(Demand::Ready(value.clone())),
            State::Failed => Ok(Demand::Failed),
        }
    }
    pub fn complete_demand(&mut self, key: DemandKey, value: Value) -> Result<()> {
        let slot = self
            .demands
            .get(&key)
            .ok_or("native demand missing from code plan")?;
        if !matches!(slot.state, State::Evaluating) {
            return Err("native demand is not evaluating".into());
        }
        self.validate(value.as_ref(), slot.ty)?;
        self.demands.get_mut(&key).unwrap().state = State::Ready(value);
        Ok(())
    }
    pub fn fail_demand(&mut self, key: DemandKey) -> Result<()> {
        let slot = self
            .demands
            .get_mut(&key)
            .ok_or("native demand missing from code plan")?;
        if !matches!(slot.state, State::Evaluating | State::Failed) {
            return Err("native demand is not evaluating".into());
        }
        slot.state = State::Failed;
        Ok(())
    }
    pub(super) fn demand_roots(&self) -> Result<Vec<Value>> {
        self.demands
            .values()
            .map(|slot| match &slot.state {
                State::Ready(value) => Ok(value.clone()),
                _ => Err("native initialization is incomplete or failed".into()),
            })
            .collect()
    }
    pub(super) fn publish_demands(&mut self, values: Vec<Value>) {
        for (slot, value) in self.demands.values_mut().zip(values) {
            slot.state = State::Ready(value);
        }
    }
}
