pub struct CallContext<'vm, 'stack> {
    current: &'vm mut Heap,
    background: Option<&'vm Heap>,
    stack: &'stack mut Vec<Option<Val>>,
    account: &'stack mut QuotaAccount,
    base: usize,
    argument_count: usize,
    upvalue_base: usize,
    upvalue_count: usize,
    result: RegisterId,
    call_site: Option<crate::Loc>,
}

#[must_use = "an opaque allocation reservation must be committed"]
pub(crate) struct OpaqueAllocationReservation(());

impl<'vm, 'stack> CallContext<'vm, 'stack> {
    pub(crate) fn make_test(&mut self, kind: crate::test_protocol::TestKind) -> Result<(), NativeError> {
        use crate::test_protocol::TestKind;
        let native_type = self
            .value(self.upvalue(0)?)?
            .as_native_type()
            .cloned()
            .ok_or_else(|| NativeError::new("Test native type is not linked"))?;
        let mut payload_bytes = 0usize;
        let source_count = if kind == TestKind::Fixtures {
            let values = self.value(self.argument(0)?)?;
            let count = values
                .sequence_len()
                .ok_or_else(|| NativeError::new("expected Array(String)"))?;
            for index in 0..count {
                let length = values
                    .sequence_get(index)
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| NativeError::new("expected fixture source String"))?
                    .as_str()
                    .len();
                payload_bytes = payload_bytes.checked_add(length).ok_or_else(|| {
                    NativeError::allocation_limit("fixture source list size overflowed")
                })?;
            }
            count
        } else {
            0
        };
        if kind == TestKind::ShouldFailWith {
            let length = self
                .value(self.argument(1)?)?
                .as_str()
                .ok_or_else(|| NativeError::new("expected String"))?
                .as_str()
                .len();
            if length == 0 {
                return Err(NativeError::new(
                    "should_fail_with requires a nonempty expectation",
                ));
            }
            payload_bytes = length;
        }
        self.charge_allocation(payload_bytes)?;
        self.charge_sequence(source_count.saturating_add(2))?;
        let mut sources = Vec::with_capacity(source_count);
        let expected = if kind == TestKind::ShouldFailWith {
            let value = self
                .value(self.argument(1)?)?
                .as_str()
                .ok_or_else(|| NativeError::new("expected String"))?
                .as_str()
                .to_owned();
            Some(value)
        } else {
            None
        };
        if kind == TestKind::Fixtures {
            let values = self.value(self.argument(0)?)?;
            let length = values
                .sequence_len()
                .ok_or_else(|| NativeError::new("expected Array(String)"))?;
            for index in 0..length {
                sources.push(
                    values
                        .sequence_get(index)
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| NativeError::new("expected fixture source String"))?
                        .as_str()
                        .to_owned(),
                );
            }
        }
        let callable =
            self.owned(self.argument(if kind == TestKind::Fixtures { 1 } else { 0 })?)?;
        let description = crate::test_protocol::TestDescription {
            kind,
            expected,
            sources,
            origin: self.call_site,
        };
        let mut value = crate::OpaqueValue::new_identity(native_type, description);
        value.traced = vec![callable].into_boxed_slice();
        let handle = self.current.allocate(Object::Opaque(value));
        self.set(
            self.result(),
            Val::unknown(DecodedValue::Opaque(handle)).with_loc(self.call_site),
        )
    }

    fn new(
        current: &'vm mut Heap,
        background: Option<&'vm Heap>,
        stack: &'stack mut Vec<Option<Val>>,
        account: &'stack mut QuotaAccount,
        arguments: Vec<Val>,
        upvalues: &[Val],
        call_site: Option<crate::Loc>,
    ) -> Result<Self, NativeError> {
        let base = stack.len();
        let argument_count = arguments.len();
        let window_size = argument_count
            .checked_add(upvalues.len())
            .and_then(|size| size.checked_add(1))
            .ok_or_else(|| NativeError::stack_limit("native stack window is too large"))?;
        let end = base
            .checked_add(window_size)
            .ok_or_else(|| NativeError::stack_limit("Telora stack size overflowed"))?;
        if end > account.stack_limit() || window_size > u32::MAX as usize {
            return Err(NativeError::stack_limit(
                "native call exceeds the Telora stack-slot limit",
            ));
        }
        stack.extend(arguments.into_iter().map(Some));
        let upvalue_base = argument_count;
        stack.extend(upvalues.iter().cloned().map(Some));
        let upvalue_count = upvalues.len();
        let result_index = argument_count + upvalue_count;
        stack.push(None);
        Ok(Self {
            current,
            background,
            stack,
            account,
            base,
            argument_count,
            upvalue_base,
            upvalue_count,
            result: RegisterId(
                u32::try_from(result_index)
                    .map_err(|_| NativeError::stack_limit("native register count exceeds u32"))?,
            ),
            call_site,
        })
    }

    pub fn argument(&self, index: usize) -> Result<RegisterId, NativeError> {
        if index >= self.argument_count {
            return Err(NativeError::new(format!(
                "argument {index} is out of bounds"
            )));
        }
        Ok(RegisterId(u32::try_from(index).map_err(|_| {
            NativeError::stack_limit("argument register exceeds u32")
        })?))
    }

    pub const fn argument_count(&self) -> usize {
        self.argument_count
    }

    pub const fn result(&self) -> RegisterId {
        self.result
    }

    pub fn upvalue(&self, index: usize) -> Result<RegisterId, NativeError> {
        if index >= self.upvalue_count {
            return Err(NativeError::new(format!(
                "upvalue {index} is out of bounds"
            )));
        }
        Ok(RegisterId(
            u32::try_from(self.upvalue_base + index)
                .map_err(|_| NativeError::stack_limit("upvalue register exceeds u32"))?,
        ))
    }

    /// The full function type selected by the static call instance. Native
    /// consumers must use this evidence rather than inspect payloads for types.
    pub fn solved_signature(&self) -> Result<Option<crate::mir::TypeId>, NativeError> {
        if self.background.is_none_or(|heap| heap.solved_types.is_none()) {
            return Ok(None);
        }
        let index = self.upvalue_count.checked_sub(1)
            .ok_or_else(|| NativeError::new("solved native closure has no signature"))?;
        self.value(self.upvalue(index)?)?.represented_type_id()
            .map(Some)
            .ok_or_else(|| NativeError::new("solved native closure has invalid signature metadata"))
    }

    pub(crate) fn solved_image(&self) -> Option<(&crate::type_image::TypeImage, &crate::execution_graph::ExecutionGraph)> {
        let heap = self.background?;
        Some((heap.solved_types.as_ref()?, heap.solved_graph.as_ref()?))
    }

    pub fn value(&self, register: RegisterId) -> Result<ValueRef<'_>, NativeError> {
        let index = usize::try_from(register.0)
            .map_err(|_| NativeError::new("register does not fit this platform"))?;
        self.stack
            .get(self.base + index)
            .and_then(Option::as_ref)
            .copied()
            .map(|value| ValueRef {
                value,
                view: HeapView {
                    current: self.current,
                    background: self.background,
                },
            })
            .ok_or_else(|| NativeError::new(format!("register {} is not initialized", register.0)))
    }

    pub fn scratch(&mut self) -> Result<RegisterId, NativeError> {
        if self.stack.len() >= self.account.stack_limit() {
            return Err(NativeError::stack_limit(
                "native scratch register exceeds the Telora stack-slot limit",
            ));
        }
        let register = RegisterId(
            u32::try_from(self.stack.len() - self.base)
                .map_err(|_| NativeError::stack_limit("native scratch register exceeds u32"))?,
        );
        self.stack.push(None);
        Ok(register)
    }

    pub fn set_atom(&mut self, destination: RegisterId, name: &str) -> Result<(), NativeError> {
        let value = self.current.atom(self.background, name);
        self.set(destination, value.into())
    }

    pub fn set_int(&mut self, destination: RegisterId, value: i64) -> Result<(), NativeError> {
        self.set(destination, DecodedValue::Int(value).into())
    }

    pub fn set_float(&mut self, destination: RegisterId, value: f64) -> Result<(), NativeError> {
        if !value.is_finite() {
            let value_count = self
                .argument_count
                .checked_add(3)
                .ok_or_else(|| NativeError::allocation_limit("allocation item count overflowed"))?;
            let bytes = logical_value_bytes(value_count)?
                .checked_add(15) // "data", "message", and "rule"
                .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))?;
            self.account
                .charge_allocation(bytes)
                .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))?;
            return Err(NativeError::non_finite_float());
        }
        self.set(destination, DecodedValue::Float(value).into())
    }

    pub fn set_none(&mut self, destination: RegisterId) -> Result<(), NativeError> {
        self.set(
            destination,
            DecodedValue::BuiltinAtom(BuiltinAtom::None).into(),
        )
    }

    pub fn set_string(
        &mut self,
        destination: RegisterId,
        value: impl Into<String>,
    ) -> Result<(), NativeError> {
        let value = value.into();
        self.charge_allocation(value.len())?;
        let value = self.current.string(self.background, &value).into();
        self.set(destination, value)
    }

    pub(crate) fn set_string_exact<F>(
        &mut self,
        destination: RegisterId,
        length: usize,
        write: F,
    ) -> Result<(), NativeError>
    where
        F: FnOnce(&mut String) -> Result<(), NativeError>,
    {
        self.charge_allocation(length)?;
        let mut value = String::new();
        value.try_reserve_exact(length).map_err(|_| {
            NativeError::allocation_limit("native String allocation cannot be reserved")
        })?;
        write(&mut value)?;
        if value.len() != length {
            return Err(NativeError::new(
                "native exact String builder produced an unexpected length",
            ));
        }
        let value = self.current.string(self.background, &value).into();
        self.set(destination, value)
    }

    pub fn set_bytes(
        &mut self,
        destination: RegisterId,
        value: impl Into<Box<[u8]>>,
    ) -> Result<(), NativeError> {
        let value = value.into();
        self.charge_allocation(value.len())?;
        let handle = self.current.allocate(Object::Bytes(value));
        self.set(destination, DecodedValue::Bytes(handle).into())
    }

    pub fn set_opaque<T>(
        &mut self,
        destination: RegisterId,
        native_type: crate::NativeType,
        payload: T,
    ) -> Result<(), NativeError>
    where
        T: std::any::Any + Eq + Send + Sync,
    {
        self.set_opaque_accounted(destination, native_type, payload, 0)
    }

    pub(crate) fn set_opaque_accounted<T>(
        &mut self,
        destination: RegisterId,
        native_type: crate::NativeType,
        payload: T,
        payload_bytes: usize,
    ) -> Result<(), NativeError>
    where
        T: std::any::Any + Eq + Send + Sync,
    {
        let reservation = self.reserve_opaque_allocation(payload_bytes)?;
        self.set_opaque_reserved(destination, native_type, payload, reservation)
    }

    pub(crate) fn reserve_opaque_allocation(
        &mut self,
        payload_bytes: usize,
    ) -> Result<OpaqueAllocationReservation, NativeError> {
        let bytes = logical_value_bytes(1)?
            .checked_add(u64::try_from(payload_bytes).map_err(|_| {
                NativeError::allocation_limit("native opaque payload size overflowed")
            })?)
            .ok_or_else(|| {
                NativeError::allocation_limit("native opaque allocation size overflowed")
            })?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))?;
        Ok(OpaqueAllocationReservation(()))
    }

    pub(crate) fn set_opaque_reserved<T>(
        &mut self,
        destination: RegisterId,
        native_type: crate::NativeType,
        payload: T,
        reservation: OpaqueAllocationReservation,
    ) -> Result<(), NativeError>
    where
        T: std::any::Any + Eq + Send + Sync,
    {
        let OpaqueAllocationReservation(()) = reservation;
        let value = crate::OpaqueValue::new(native_type, payload);
        let handle = self.current.allocate(Object::Opaque(value));
        self.set(destination, DecodedValue::Opaque(handle).into())
    }

    pub fn set_identity_opaque<T>(
        &mut self,
        destination: RegisterId,
        native_type: crate::NativeType,
        payload: T,
    ) -> Result<(), NativeError>
    where
        T: std::any::Any + Send + Sync,
    {
        self.charge_sequence(1)?;
        let value = crate::OpaqueValue::new_identity(native_type, payload);
        let handle = self.current.allocate(Object::Opaque(value));
        self.set(destination, DecodedValue::Opaque(handle).into())
    }

    pub fn copy(&mut self, destination: RegisterId, source: RegisterId) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        self.set(destination, value)
    }

    pub fn copy_field(
        &mut self,
        destination: RegisterId,
        source: RegisterId,
        field: &str,
    ) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        let DecodedValue::Dict(handle) = value.value() else {
            return Err(NativeError::new("native field source must be a Dict"));
        };
        let value = HeapView {
            current: self.current,
            background: self.background,
        }
        .dict_get_text(handle, field)
        .map_err(|error| NativeError::new(error.to_string()))?
        .ok_or_else(|| NativeError::new(format!("native field source has no field {field:?}")))?;
        self.set(destination, value)
    }

    pub fn copy_sequence_item(
        &mut self,
        destination: RegisterId,
        source: RegisterId,
        index: usize,
    ) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        let DecodedValue::Array(handle) = value.value() else {
            return Err(NativeError::new("native sequence source must be an Array"));
        };
        let value = HeapView {
            current: self.current,
            background: self.background,
        }
        .sequence(handle, false)
        .map_err(|error| NativeError::new(error.to_string()))?
        .get(index)
        .copied()
        .ok_or_else(|| NativeError::new(format!("native sequence has no item {index}")))?;
        self.set(destination, value)
    }

    pub fn copy_tagged_payload(
        &mut self,
        destination: RegisterId,
        source: RegisterId,
    ) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        let DecodedValue::Tagged(handle) = value.value() else {
            return Err(NativeError::new("native tagged source must have a payload"));
        };
        let (_, payload) = HeapView {
            current: self.current,
            background: self.background,
        }
        .tagged(handle)
        .map_err(|error| NativeError::new(error.to_string()))?;
        self.set(destination, payload)
    }

    pub fn set_semantic_value(
        &mut self,
        destination: RegisterId,
        source: &DataWorld,
        owner: RegisterId,
        allocation_hint: usize,
    ) -> Result<(), NativeError> {
        let background = self
            .background
            .ok_or_else(|| NativeError::new("semantic Value requires a Main world"))?;
        let owner = self.owned(owner)?;
        self.charge_allocation(allocation_hint)?;
        let raw = source
            .relocate_into(self.current, background)
            .map_err(|error| NativeError::new(error.to_string()))?;
        let wrapper_bytes = semantic_value_wrapper_bytes(self.current, Some(background), raw)
            .map_err(|error| NativeError::new(error.to_string()))?;
        self.account
            .charge_allocation(wrapper_bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))?;
        let value = wrap_semantic_value(self.current, Some(background), raw, owner)
            .map_err(|error| NativeError::new(error.to_string()))?;
        self.set(destination, value)
    }

    pub fn make_array(
        &mut self,
        destination: RegisterId,
        items: &[RegisterId],
    ) -> Result<(), NativeError> {
        let values = items
            .iter()
            .map(|item| self.owned(*item))
            .collect::<Result<Vec<_>, _>>()?;
        self.charge_sequence(values.len())?;
        let value = Val::unknown(DecodedValue::Array(
            self.current.allocate(Object::Array(values.into())),
        ));
        self.set(destination, value)
    }

    pub fn make_tuple(
        &mut self,
        destination: RegisterId,
        items: &[RegisterId],
    ) -> Result<(), NativeError> {
        let values = items
            .iter()
            .map(|item| self.owned(*item))
            .collect::<Result<Vec<_>, _>>()?;
        self.charge_sequence(values.len())?;
        let value = Val::unknown(DecodedValue::Tuple(
            self.current.allocate(Object::Tuple(values.into())),
        ));
        self.set(destination, value)
    }

    pub fn make_tagged(
        &mut self,
        destination: RegisterId,
        tag: RegisterId,
        payload: RegisterId,
    ) -> Result<(), NativeError> {
        let tag = self.owned(tag)?;
        let payload = self.owned(payload)?;
        self.charge_sequence(2)?;
        let value = Val::unknown(DecodedValue::Tagged(
            self.current.allocate(Object::Tagged { tag, payload }),
        ));
        self.set(destination, value)
    }

    pub fn make_dict(
        &mut self,
        destination: RegisterId,
        fields: &[(String, RegisterId)],
    ) -> Result<(), NativeError> {
        let mut entries = fields
            .iter()
            .map(|(name, register)| Ok((name.as_str(), self.owned(*register)?)))
            .collect::<Result<Vec<_>, NativeError>>()?;
        self.charge_dict(&entries)?;
        entries.sort_by(|left, right| left.0.cmp(right.0));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(NativeError::new("Dict contains a duplicate field"));
        }
        let (fields, values): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .map(|(field, value)| (self.current.intern(field), value))
            .unzip();
        let shape = self.current.intern_shape(fields);
        let value = Val::unknown(DecodedValue::Dict(self.current.allocate(Object::Dict {
            shape,
            values: values.into(),
        })));
        self.set(destination, value)
    }

    fn charge_sequence(&mut self, count: usize) -> Result<(), NativeError> {
        let bytes = logical_value_bytes(count)?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))
    }

    fn charge_dict(&mut self, entries: &[(&str, Val)]) -> Result<(), NativeError> {
        let field_bytes = entries.iter().try_fold(0u64, |total, (field, _)| {
            total.checked_add(field.len() as u64).ok_or_else(|| {
                NativeError::allocation_limit("native Dict allocation size overflowed")
            })
        })?;
        let value_bytes = logical_value_bytes(entries.len())?;
        let bytes = field_bytes.checked_add(value_bytes).ok_or_else(|| {
            NativeError::allocation_limit("native Dict allocation size overflowed")
        })?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))
    }

    fn charge_allocation(&mut self, bytes: usize) -> Result<(), NativeError> {
        let bytes = u64::try_from(bytes)
            .map_err(|_| NativeError::allocation_limit("native allocation size overflowed"))?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))
    }

    fn owned(&self, register: RegisterId) -> Result<Val, NativeError> {
        let index = usize::try_from(register.0)
            .map_err(|_| NativeError::new("register does not fit this platform"))?;
        self.stack
            .get(self.base + index)
            .and_then(Option::as_ref)
            .copied()
            .ok_or_else(|| NativeError::new(format!("register {} is not initialized", register.0)))
    }

    fn set(&mut self, register: RegisterId, value: Val) -> Result<(), NativeError> {
        let index = usize::try_from(register.0)
            .map_err(|_| NativeError::new("register does not fit this platform"))?;
        let slot = self
            .stack
            .get_mut(self.base + index)
            .ok_or_else(|| NativeError::new(format!("register {} is out of bounds", register.0)))?;
        *slot = Some(value);
        Ok(())
    }

    fn take_result(self) -> Result<Val, NativeError> {
        let index = usize::try_from(self.result.0)
            .map_err(|_| NativeError::stack_limit("result register does not fit usize"))?;
        let slot = self
            .base
            .checked_add(index)
            .and_then(|slot| self.stack.get_mut(slot))
            .ok_or_else(|| NativeError::new("native result register is out of bounds"))?;
        let result = slot
            .take()
            .ok_or_else(|| NativeError::new("native function did not write its result register"));
        self.stack.truncate(self.base);
        result
    }
}
