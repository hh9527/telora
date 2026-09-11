impl Heap {
    pub(crate) fn record_value(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<Val, HeapError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let fields = entries
            .iter()
            .map(|(name, _)| self.intern(name))
            .collect::<Vec<_>>();
        let values = entries
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        let shape = self.intern_shape(fields);
        Ok(Val::unknown(DecodedValue::Dict(self.allocate(
            Object::Dict {
                shape,
                values: values.into_boxed_slice(),
            },
        ))))
    }

    pub(crate) fn native_closure(
        &mut self,
        function: NativeFunction,
        upvalues: impl Into<Box<[Val]>>,
    ) -> Val {
        let handle = self.allocate(Object::Closure {
            identity: Arc::new(()),
            prototype: RuntimePrototype::Native(function),
            upvalues: upvalues.into(),
        });
        Val::unknown(DecodedValue::Func(handle))
    }

    pub(crate) fn native_type_value(&mut self, value: crate::NativeType) -> Val {
        let id = self.intern_native_type(value);
        Val::unknown(DecodedValue::NativeType(id))
    }

}
