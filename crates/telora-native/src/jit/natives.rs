use super::*;

impl Lower<'_, '_> {
    pub(super) fn native_adapter(
        &mut self,
        node: HirId,
        arguments: &[TypeKey],
        data: ir::Value,
    ) -> EmitResult<()> {
        let symbol =
            self.mir.hir_symbols[node.index()].ok_or("native declaration has no stable symbol")?;
        let declaration = &self.mir.symbols[symbol.index()];
        let module = declaration
            .module
            .and_then(|id| self.mir.modules[id.index()].native.as_ref())
            .ok_or("native declaration is not in an admitted ABI module")?;
        if (module.id, declaration.name.as_str()) == (5, "map") {
            if arguments.len() != 2 {
                return Err("native map arity mismatch".into());
            }
            let array = &self.mir.types[arguments[0].index()];
            let callback = &self.mir.types[arguments[1].index()];
            let output = &self.mir.types[self.return_type.index()];
            if array.constructor != TypeConstructor::Array
                || output.constructor != TypeConstructor::Array
                || callback.constructor != TypeConstructor::Function
                || callback.arguments.len() != 2
                || array.arguments != callback.arguments[..1]
                || output.arguments != callback.arguments[1..]
            {
                return Err("native map does not match its closed callback signature".into());
            }
            let dispatcher = self.functions.dispatcher(arguments[1], self.module)?;
            let dispatcher = self
                .module
                .declare_func_in_func(dispatcher, self.builder.func);
            let address = self
                .builder
                .ins()
                .func_addr(self.module.target_config().pointer_type(), dispatcher);
            let mut packet = vec![address];
            let width = self.layouts.words(arguments[0])? + self.layouts.words(arguments[1])?;
            for i in 0..width {
                packet.push(self.builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    data,
                    (i * 8) as i32,
                ));
            }
            let packet = self.stack_words(&packet)?;
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::ARRAY_MAP, self.return_type, packet, count)?;
            return self.write_return(&value);
        }
        // These are ABI export keys of an admitted module, not spellings at a
        // source call site. Renaming an import does not change its linked symbol.
        let (operation, input) = match (module.id, declaration.name.as_str()) {
            (5, "length") => (helpers::ARRAY_LENGTH, TypeConstructor::Array),
            (7, "length") => (helpers::STRING_LENGTH, TypeConstructor::String),
            _ => {
                return Err(format!(
                    "native ABI not implemented: module {}, export {}",
                    module.id, declaration.name
                )
                .into());
            }
        };
        if arguments.len() != 1
            || self.mir.types[arguments[0].index()].constructor != input
            || self.mir.types[self.return_type.index()].constructor != TypeConstructor::Int
        {
            return Err("native ABI does not match its sealed declaration signature".into());
        }
        let count = self.builder.ins().iconst(types::I64, 0);
        let result = self.object(node, operation, self.return_type, data, count)?;
        self.write_return(&result)
    }
}
