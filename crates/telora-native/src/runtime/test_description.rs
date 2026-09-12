use super::*;

#[derive(Clone)]
pub(super) struct TestDescription {
    pub operation: usize,
    pub inputs: Vec<Box<[u64]>>,
}

impl Runtime {
    pub(super) fn test_description(&self, value: &Value) -> Result<&TestDescription> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Test)?;
        let reference = HeapRef::from_raw(u32::try_from(value.words()[2]).map_err(|_| "invalid Test HeapId")?);
        let description = self.tables(reference).tests.get(reference.slot() as usize).ok_or("invalid Test HeapId")?;
        if description.operation > 3 { return Err("invalid test description operation".into()); }
        Ok(description)
    }
    pub(super) fn make_test(&mut self, ty: TypeId, loc: Location, operation: usize, inputs: &[Value]) -> Result<Value> {
        self.expect(ty, Kind::Test)?;
        if operation > 3 || inputs.len() != if operation < 2 { 1 } else { 2 } { return Err("invalid test description arity".into()); }
        self.function_id(&inputs[usize::from(operation == 3)])?;
        if operation == 2 && self.text(inputs[1].as_ref())?.as_str().is_empty() { return Err("should_fail_with requires a nonempty expectation".into()); }
        if operation == 3 {
            let value_ty = self.data_contract.as_ref().ok_or("test fixture Value contract is not loaded")?.value_type();
            if self.layout(inputs[1].type_id())?.arguments.first() != Some(&value_ty) { return Err("fixture callback does not consume the sealed Value identity".into()); }
            for index in 0..self.array_len(&inputs[0])? { self.text(self.array_get(&inputs[0], index)?)?; }
        }
        let id = HeapRef::new(World::Work, u32::try_from(self.work.tests.len()).map_err(|_| "Test table overflow")?)?;
        self.work.tests.push(TestDescription { operation, inputs: inputs.iter().map(|value| value.words().into()).collect() });
        self.pack(ty, loc, &[u64::from(id.raw())])
    }
}
