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
        if module.id == 2 && matches!(declaration.name.as_str(), "pack" | "desc" | "project_with") {
            let kind = |ty: TypeKey| &self.mir.types[ty.index()].constructor;
            let operation = match declaration.name.as_str() {
                "desc"
                    if arguments.len() == 1
                        && kind(arguments[0]) == &TypeConstructor::Dyn
                        && kind(self.return_type) == &TypeConstructor::Type =>
                {
                    helpers::DYN_DESC
                }
                "pack"
                    if arguments.len() == 2
                        && kind(arguments[0]) == &TypeConstructor::TypeOf
                        && self.mir.types[arguments[0].index()]
                            .arguments
                            .first()
                            .is_some_and(|ty| ty.index() == arguments[1].index())
                        && kind(self.return_type) == &TypeConstructor::Dyn =>
                {
                    helpers::DYN_PACK
                }
                "project_with"
                    if arguments.len() == 2
                        && kind(arguments[0]) == &TypeConstructor::TypeOf
                        && kind(arguments[1]) == &TypeConstructor::Dyn
                        && kind(self.return_type) == &TypeConstructor::Option
                        && self.mir.types[arguments[0].index()].arguments
                            == self.mir.types[self.return_type.index()].arguments =>
                {
                    helpers::DYN_PROJECT
                }
                _ => return Err("native Dyn ABI signature mismatch".into()),
            };
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, operation, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 2
            && matches!(
                declaration.name.as_str(),
                "check_int" | "check_float" | "check_string" | "check_bytes"
            )
        {
            let expected = match declaration.name.as_str() {
                "check_int" => TypeConstructor::Int,
                "check_float" => TypeConstructor::Float,
                "check_string" => TypeConstructor::String,
                _ => TypeConstructor::Bytes,
            };
            let output = &self.mir.types[self.return_type.index()];
            if arguments.len() != 1
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Dyn
                || output.constructor != TypeConstructor::Option
                || output.arguments.len() != 1
                || self.mir.types[output.arguments[0].index()].constructor != expected
            {
                return Err("native Dyn scalar check signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::DYN_CHECK, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 2 && declaration.name == "kind" {
            let expected = [
                "Array", "Atom", "Bytes", "Dict", "Dyn", "Float", "Func", "Int", "Opaque",
                "String", "Tagged", "Tuple", "Type",
            ];
            if arguments.len() != 1
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Dyn
                || !matches!(self.mir.types[self.return_type.index()].constructor, TypeConstructor::Nominal(symbol)
                    if self.mir.type_definitions.iter().find(|d| d.symbol == symbol).is_some_and(|d|
                        d.members.iter().map(|m| m.name.as_str()).eq(expected)))
                || self.layouts.variant_payloads[self.return_type.index()]
                    .iter()
                    .any(Option::is_some)
            {
                return Err("native Dyn kind ABI signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::DYN_KIND, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 2 && declaration.name == "field_raw" {
            let output = &self.mir.types[self.return_type.index()];
            if arguments.len() != 2
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Dyn
                || self.mir.types[arguments[1].index()].constructor != TypeConstructor::String
                || output.constructor != TypeConstructor::Result
                || output.arguments.len() != 2
                || self.mir.types[output.arguments[0].index()].constructor != TypeConstructor::Dyn
                || self.mir.types[output.arguments[1].index()].constructor
                    != TypeConstructor::String
            {
                return Err("native Dyn field ABI signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::DYN_FIELD, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 2
            && matches!(
                declaration.name.as_str(),
                "fields_raw" | "array_items_raw" | "tuple_items_raw" | "tag_raw" | "payload_raw"
            )
        {
            let query = match declaration.name.as_str() {
                "fields_raw" => 0,
                "array_items_raw" => 1,
                "tuple_items_raw" => 2,
                "tag_raw" => 3,
                _ => 4,
            };
            let result = &self.mir.types[self.return_type.index()];
            if arguments.len() != 1
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Dyn
                || result.constructor != TypeConstructor::Result
                || result.arguments.len() != 2
                || self.mir.types[result.arguments[1].index()].constructor
                    != TypeConstructor::String
            {
                return Err("native Dyn query Result signature mismatch".into());
            }
            let output = &self.mir.types[result.arguments[0].index()];
            let valid = match query {
                3 => output.constructor == TypeConstructor::String,
                4 => {
                    output.constructor == TypeConstructor::Option
                        && output.arguments.len() == 1
                        && self.mir.types[output.arguments[0].index()].constructor
                            == TypeConstructor::Dyn
                }
                _ if output.constructor == TypeConstructor::Array
                    && output.arguments.len() == 1 =>
                {
                    let element = &self.mir.types[output.arguments[0].index()];
                    if query == 0 {
                        element.constructor == TypeConstructor::Tuple
                            && element.arguments.len() == 2
                            && self.mir.types[element.arguments[0].index()].constructor
                                == TypeConstructor::String
                            && self.mir.types[element.arguments[1].index()].constructor
                                == TypeConstructor::Dyn
                    } else {
                        element.constructor == TypeConstructor::Dyn
                    }
                }
                _ => false,
            };
            if !valid {
                return Err("native Dyn query payload signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, query);
            let value = self.object(node, helpers::DYN_QUERY, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 2
            && matches!(
                declaration.name.as_str(),
                "get_field_value" | "get_variant_index" | "get_variant_payload"
            )
        {
            let operation = match declaration.name.as_str() {
                "get_field_value" => 0,
                "get_variant_index" => 1,
                _ => 2,
            };
            let output = &self.mir.types[self.return_type.index()];
            let valid = match operation {
                0 => output.constructor == TypeConstructor::Dyn,
                1 => output.constructor == TypeConstructor::Int,
                _ => {
                    output.constructor == TypeConstructor::Option
                        && output.arguments.len() == 1
                        && self.mir.types[output.arguments[0].index()].constructor
                            == TypeConstructor::Dyn
                }
            };
            if arguments.len() != if operation == 1 { 1 } else { 2 }
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Dyn
                || (operation != 1
                    && self.mir.types[arguments[1].index()].constructor != TypeConstructor::Int)
                || !valid
            {
                return Err("native Dyn indexed access ABI mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, operation);
            let value = self.object(node, helpers::DYN_MEMBER, self.return_type, data, count)?;
            return self.write_return(&value);
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
