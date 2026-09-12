use super::*;

impl Lower<'_, '_> {
    fn property_factory(
        &mut self,
        node: HirId,
        arguments: &[TypeKey],
        data: ir::Value,
        environment: ir::Value,
    ) -> EmitResult<()> {
        let factory = &self.mir.types[known(self.mir, node)?.index()];
        if factory.constructor != TypeConstructor::Function || factory.arguments.len() != 2 {
            return Err("property factory ABI signature mismatch".into());
        }
        let target = TypeKey::try_from(factory.arguments[0])?;
        let provider = TypeKey::try_from(factory.arguments[1])?;
        if self.mir.types[target.index()].constructor != TypeConstructor::PropertyTarget {
            return Err("property factory target ABI mismatch".into());
        }
        if !self.function_key.marker_provider {
            if arguments != [target] || self.return_type != provider {
                return Err("property factory ABI mismatch".into());
            }
            let function = self.functions.declare(
                self.mir,
                functions::Key {
                    node,
                    instance: None,
                    initializer: false,
                    marker_provider: true,
                },
                self.module,
            )?;
            let mut words = vec![
                self.builder
                    .ins()
                    .iconst(types::I64, i64::from(function.as_u32())),
            ];
            for i in 0..self.layouts.words(target)? {
                words.push(self.builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    data,
                    (i * 8) as i32,
                ));
            }
            let count = self.builder.ins().iconst(types::I64, words.len() as i64);
            let data = self.stack_words(&words)?;
            let result = self.object(node, helpers::CLOSURE, provider, data, count)?;
            return self.write_return(&result);
        }
        if arguments.len() != 2
            || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Type
            || self.mir.types[arguments[1].index()].constructor != TypeConstructor::Option
            || self.mir.types[arguments[1].index()].arguments
                != [self
                    .ty(node)
                    .map(|ty| self.mir.types[ty.index()].arguments[2])?]
            || self.layouts.field_names[self.return_type.index()] != ["bits"]
        {
            return Err("property provider ABI signature mismatch".into());
        }
        let zero = self.builder.ins().iconst(types::I64, 0);
        let mut packet = self.object(node, helpers::CAPTURE, target, environment, zero)?;
        let start = self.layouts.words(arguments[0])?;
        for i in 0..self.layouts.words(arguments[1])? {
            packet.push(self.builder.ins().load(
                types::I64,
                MemFlagsData::new(),
                data,
                ((start + i) * 8) as i32,
            ));
        }
        let data = self.stack_words(&packet)?;
        let result = self.object(node, helpers::PROPERTY_MARK, self.return_type, data, zero)?;
        self.write_return(&result)
    }
    pub(super) fn native_adapter(
        &mut self,
        node: HirId,
        arguments: &[TypeKey],
        data: ir::Value,
        environment: ir::Value,
    ) -> EmitResult<()> {
        let symbol =
            self.mir.hir_symbols[node.index()].ok_or("native declaration has no stable symbol")?;
        let declaration = &self.mir.symbols[symbol.index()];
        let module = declaration
            .module
            .and_then(|id| self.mir.modules[id.index()].native.as_ref())
            .ok_or("native declaration is not in an admitted ABI module")?;
        if (module.id, declaration.name.as_str()) == (18, "property") {
            return self.property_factory(node, arguments, data, environment);
        }
        if module.id == 25
            && matches!(
                declaration.name.as_str(),
                "get_type_prop" | "evidence" | "get_field_prop" | "get_variant_prop"
            )
        {
            let site = match declaration.name.as_str() {
                "get_field_prop" => telora_core::mir::PropertySite::Field(0),
                "get_variant_prop" => telora_core::mir::PropertySite::Variant(0),
                _ => telora_core::mir::PropertySite::Type,
            };
            return self.property_query(
                node,
                arguments,
                data,
                declaration.name == "evidence",
                site,
            );
        }
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
