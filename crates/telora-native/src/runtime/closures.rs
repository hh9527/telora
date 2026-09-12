use super::*;

// Not a code-plan ID: the environment contains a single function descriptor
// once filled. An empty environment denotes a pending lexical function slot.
pub(super) const FUNCTION_SLOT: u32 = u32::MAX;

impl Runtime {
    pub(crate) fn reserve_function(&mut self, ty: TypeId, loc: Location) -> Result<Value> {
        self.expect(ty, Kind::Function)?;
        let environment = self.push_words(Table::Environments, vec![])?
            .checked_add(1).ok_or("environment ID overflow")?;
        self.pack(ty, loc, &[u64::from(FUNCTION_SLOT) | (u64::from(environment) << 32)])
    }

    pub(crate) fn fill_function(&mut self, target: &Value, source: &Value) -> Result<()> {
        if self.function_id(target)? != FUNCTION_SLOT {
            return Err("function target is not a reserved slot".into());
        }
        self.validate(source.as_ref(), target.type_id())?;
        let raw = ((target.words[2] >> 32) as u32).checked_sub(1)
            .ok_or("function slot has no identity")?;
        let reference = HeapRef::from_raw(raw);
        if reference.world() != World::Work {
            return Err("cannot fill a published function slot".into());
        }
        if !self.object_words(Table::Environments, raw)?.is_empty() {
            return Err("function slot is already filled".into());
        }
        if target.words[2] == source.words[2] {
            return Err("function slot cannot refer directly to itself".into());
        }
        self.charge_allocation(source.words().len(), 8, 0)?;
        self.work.environments.entries[reference.slot() as usize].words = source.words().to_vec();
        Ok(())
    }

    /// Follow lexical slots only at invocation. Equality and captured aliases
    /// retain the slot descriptor, so installing a body never changes identity.
    pub(crate) fn resolve_function(&self, value: &Value) -> Result<Value> {
        let mut current = value.clone();
        let limit = self.main.environments.entries.len()
            .saturating_add(self.work.environments.entries.len());
        for _ in 0..=limit {
            if self.function_id(&current)? != FUNCTION_SLOT { return Ok(current); }
            let raw = ((current.words[2] >> 32) as u32).checked_sub(1)
                .ok_or("function slot has no identity")?;
            let words = self.object_words(Table::Environments, raw)?;
            if words.is_empty() { return Err("function called before its declaration was initialized".into()); }
            let next = Value { arena: self.identity, words: words.into() };
            self.validate(next.as_ref(), value.type_id())?;
            current = next;
        }
        Err("function slot alias cycle".into())
    }
    /// Each adapter retains the factory descriptor followed by its witnesses.
    /// Cache by runtime factory identity and represented types, never locations.
    pub(crate) fn interpreter_adapter(
        &mut self, factory: &Value, witnesses: &[Value], ty: TypeId, function: u32, loc: Location,
    ) -> Result<Value> {
        self.function_id(factory)?;
        self.expect(ty, Kind::Function)?;
        let signature = &self.layout(factory.type_id())?.arguments;
        let (output, parameters) = signature.split_last().ok_or("interpreter factory has no signature")?;
        if *output != ty || parameters.len() != witnesses.len() {
            return Err("interpreter factory signature mismatch".into());
        }
        for (witness, parameter) in witnesses.iter().zip(parameters) {
            self.validate(witness.as_ref(), *parameter)?;
        }
        let types = witnesses.iter().map(|value| self.represented_type(value.as_ref()))
            .collect::<Result<Vec<_>>>()?;
        let key = (factory.words[2], types);
        if let Some(adapter) = self.interpreter_adapters.get(&key) {
            if adapter.type_id() != ty || self.function_id(adapter)? != function {
                return Err("interpreter adapter contradicts its sealed specialization".into());
            }
            return Ok(adapter.clone());
        }
        // Charge persistent cache metadata before creating the environment.
        self.charge_allocation(witnesses.len(), std::mem::size_of::<TypeId>(),
            std::mem::size_of::<((u64, Vec<TypeId>), Value)>() + 3 * 8)?;
        let mut captures = Vec::with_capacity(1 + witnesses.len());
        captures.push(factory.clone());
        captures.extend_from_slice(witnesses);
        let adapter = self.closure(ty, loc, function, &captures)?;
        self.interpreter_adapters.insert(key, adapter.clone());
        Ok(adapter)
    }
    /// FunctionId is a code-plan identity, never a machine address. Environment
    /// IDs encode HeapRef + 1. Even an empty environment owns a fresh closure
    /// identity; FunctionId alone identifies code, not a runtime function value.
    pub fn closure(
        &mut self,
        ty: TypeId,
        loc: Location,
        function: u32,
        captures: &[Value],
    ) -> Result<Value> {
        self.expect(ty, Kind::Function)?;
        if function == FUNCTION_SLOT { return Err("reserved function code ID".into()); }
        let mut words = Vec::new();
        for capture in captures {
            self.validate(capture.as_ref(), capture.type_id())?;
            words.extend_from_slice(capture.words());
        }
        let environment = self.push_words(Table::Environments, words)?
            .checked_add(1).ok_or("environment ID overflow")?;
        self.pack(
            ty,
            loc,
            &[u64::from(function) | (u64::from(environment) << 32)],
        )
    }
    pub fn function_id(&self, value: &Value) -> Result<u32> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Function)?;
        Ok(value.words[2] as u32)
    }
    pub fn capture(&self, value: &Value, index: usize) -> Result<ValueRef<'_>> {
        self.function_id(value)?;
        let environment = (value.words[2] >> 32) as u32;
        let raw = environment
            .checked_sub(1)
            .ok_or("closure has no captures")?;
        let mut words = self.object_words(Table::Environments, raw)?;
        for current in 0..=index {
            let header = words.get(1).ok_or("capture index outside environment")?;
            let ty = TypeId((header >> 32) as u32);
            let width = self.layout(ty)?.words;
            let capture = words.get(..width).ok_or("truncated closure capture")?;
            if current == index {
                return Ok(ValueRef {
                    arena: self.identity,
                    words: capture,
                });
            }
            words = &words[width..];
        }
        unreachable!()
    }
}
