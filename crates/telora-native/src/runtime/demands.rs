use super::*;
use telora_core::mir::{PropertySite, SymbolId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DemandKey {
    Export(SymbolId),
    Instance(telora_core::mir::GenericInstanceId),
    ConstructionCheck { owner: TypeId, site: PropertySite },
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
    pub(super) ty: TypeId,
    state: State,
}
#[derive(Debug)]
pub enum Demand {
    Evaluate,
    Ready(Value),
    Failed,
}

impl Runtime {
    pub(super) fn demand_width(&self, ty: TypeId) -> Result<usize> {
        if self.type_info.get(ty.index()).is_some_and(|info| info.kind == Some("Never")) {
            Ok(0)
        } else {
            Ok(self.layout(ty)?.words)
        }
    }
    pub fn register_demand(&mut self, key: DemandKey, ty: TypeId) -> Result<()> {
        if self.published {
            return Err("main world is already sealed".into());
        }
        // Never has no value layout, but its initializer can be demanded and
        // fail. It can never transition to Ready or be published.
        self.demand_width(ty)?;
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
