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
        if !self.function_key.configured_factory {
            if arguments != [target] || self.return_type != provider {
                return Err("property factory ABI mismatch".into());
            }
            let function = self.functions.declare(
                self.mir,
                functions::Key {
                    node,
                    instance: None,
                    initializer: false,
                    configured_factory: true,
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
        if module.id == 20
            && matches!(
                declaration.name.as_str(),
                "prepare" | "from_string" | "from_int" | "from_float" | "concat" | "render"
            )
        {
            let is_fmt = |ty: TypeKey| {
                matches!(self.mir.types[ty.index()].constructor,
                TypeConstructor::Native(native) if (native.module, native.slot) == (20, 1))
            };
            let kind = |ty: TypeKey| &self.mir.types[ty.index()].constructor;
            let array = |ty: TypeKey, fmt: bool| {
                let ty = &self.mir.types[ty.index()];
                ty.constructor == TypeConstructor::Array
                    && ty.arguments.len() == 1
                    && if fmt {
                        matches!(self.mir.types[ty.arguments[0].index()].constructor,
                        TypeConstructor::Native(native) if (native.module, native.slot) == (20, 1))
                    } else {
                        self.mir.types[ty.arguments[0].index()].constructor
                            == TypeConstructor::String
                    }
            };
            let operation = match declaration.name.as_str() {
                "prepare" => 0,
                "from_string" => 1,
                "from_int" => 2,
                "from_float" => 3,
                "concat" => 4,
                _ => 5,
            };
            let output = &self.mir.types[self.return_type.index()];
            let valid = if operation == 4 {
                arguments.len() == 2
                    && array(arguments[0], false)
                    && array(arguments[1], true)
                    && is_fmt(self.return_type)
            } else if arguments.len() != 1 {
                false
            } else {
                match operation {
                    0 => {
                        kind(arguments[0]) == &TypeConstructor::String
                            && output.constructor == TypeConstructor::Tuple
                            && output.arguments.len() == 2
                            && output
                                .arguments
                                .iter()
                                .all(|&ty| TypeKey::try_from(ty).is_ok_and(|ty| array(ty, false)))
                    }
                    1 => kind(arguments[0]) == &TypeConstructor::String && is_fmt(self.return_type),
                    2 => kind(arguments[0]) == &TypeConstructor::Int && is_fmt(self.return_type),
                    3 => kind(arguments[0]) == &TypeConstructor::Float && is_fmt(self.return_type),
                    _ => is_fmt(arguments[0]) && output.constructor == TypeConstructor::String,
                }
            };
            if !valid {
                return Err("native Fmt ABI signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, operation);
            let result = self.object(node, helpers::FORMAT, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        if module.id == 5 && let Some(operation) = ["enumerate", "push", "concat", "zip"].iter().position(|name| *name == declaration.name) {
            if arguments.len() != if operation == 1 || operation == 3 { 2 } else { 1 } { return Err("native array construction arity mismatch".into()); }
            let input = &self.mir.types[arguments[0].index()];
            let output = &self.mir.types[self.return_type.index()];
            let valid = input.constructor == TypeConstructor::Array && input.arguments.len() == 1 && match operation {
                0 => output.constructor == TypeConstructor::Array && output.arguments.len() == 1 && {
                    let pair = &self.mir.types[output.arguments[0].index()];
                    pair.constructor == TypeConstructor::Tuple && pair.arguments.len() == 2 && self.mir.types[pair.arguments[0].index()].constructor == TypeConstructor::Int && pair.arguments[1] == input.arguments[0]
                },
                1 => arguments[0] == self.return_type && input.arguments[0].index() == arguments[1].index(),
                2 => input.arguments[0].index() == self.return_type.index() && output.constructor == TypeConstructor::Array,
                _ => output.constructor == TypeConstructor::Option && output.arguments.len() == 1 && {
                    let array = &self.mir.types[output.arguments[0].index()];
                    let right = &self.mir.types[arguments[1].index()];
                    array.constructor == TypeConstructor::Array && array.arguments.len() == 1 && right.constructor == TypeConstructor::Array && right.arguments.len() == 1 && {
                        let pair = &self.mir.types[array.arguments[0].index()];
                        pair.constructor == TypeConstructor::Tuple && pair.arguments == [input.arguments[0], right.arguments[0]]
                    }
                },
            };
            if !valid { return Err("native array construction signature mismatch".into()); }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let value = self.object(node, helpers::ARRAY_BUILD, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if (module.id, declaration.name.as_str()) == (5, "get") {
            if arguments.len() != 2 { return Err("native Array.get arity mismatch".into()); }
            let input = &self.mir.types[arguments[0].index()];
            let output = &self.mir.types[self.return_type.index()];
            if input.constructor != TypeConstructor::Array || self.mir.types[arguments[1].index()].constructor != TypeConstructor::Int || output.constructor != TypeConstructor::Option || output.arguments != input.arguments {
                return Err("native Array.get signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::ARRAY_GET, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 6 && let Some(operation) = ["get", "keys", "values", "pairs", "from_pairs", "merge"].iter().position(|name| *name == declaration.name) {
            if arguments.len() != if operation == 0 || operation == 5 { 2 } else { 1 } {
                return Err("native dictionary read arity mismatch".into());
            }
            let input = &self.mir.types[arguments[0].index()];
            let output = &self.mir.types[self.return_type.index()];
            let pair_matches = |array: &telora_core::mir::ResolvedType, dict: &telora_core::mir::ResolvedType| {
                array.constructor == TypeConstructor::Array && dict.constructor == TypeConstructor::Dict && array.arguments.len() == 1 && {
                    let pair = &self.mir.types[array.arguments[0].index()];
                    pair.constructor == TypeConstructor::Tuple && pair.arguments.len() == 2 && self.mir.types[pair.arguments[0].index()].constructor == TypeConstructor::String && pair.arguments[1..] == dict.arguments
                }
            };
            let valid = match operation {
                3 => pair_matches(output, input),
                4 => pair_matches(input, output),
                5 => input.constructor == TypeConstructor::Dict && arguments[0] == arguments[1] && arguments[0] == self.return_type,
                _ => input.constructor == TypeConstructor::Dict && match operation {
                0 => output.constructor == TypeConstructor::Option && output.arguments == input.arguments && self.mir.types[arguments[1].index()].constructor == TypeConstructor::String,
                1 => output.constructor == TypeConstructor::Array && output.arguments.len() == 1 && self.mir.types[output.arguments[0].index()].constructor == TypeConstructor::String,
                _ => output.constructor == TypeConstructor::Array && output.arguments == input.arguments,
                },
            };
            if !valid { return Err("native dictionary read signature mismatch".into()); }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let result = self.object(node, helpers::DICT_READ, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        if (module.id, declaration.name.as_str()) == (13, "decode_with") {
            let output = &self.mir.types[self.return_type.index()];
            if arguments.len() != 3
                || self.mir.types[arguments[1].index()].constructor != TypeConstructor::TypeOf
                || self.mir.types[arguments[1].index()].arguments.len() != 1
                || output.constructor != TypeConstructor::Result
                || output.arguments.len() != 2
                || output.arguments[0] != self.mir.types[arguments[1].index()].arguments[0]
            { return Err("native codec decode signature mismatch".into()); }
            let (packet, count) = self.codec_packet(data, TypeKey::try_from(output.arguments[0])?, true)?;
            let result = self.object(node, helpers::DECODE, self.return_type, packet, count)?;
            return self.write_return(&result);
        }
        if (module.id, declaration.name.as_str()) == (13, "encode_with") {
            if arguments.len() != 3
                || self.mir.types[arguments[1].index()].constructor != TypeConstructor::TypeOf
                || self.mir.types[arguments[1].index()].arguments.len() != 1
                || self.mir.types[arguments[1].index()].arguments[0].index() != self.return_type.index()
            { return Err("native codec encode signature mismatch".into()); }
            let (packet, count) = self.codec_packet(data, arguments[2], false)?;
            let result = self.object(node, helpers::ENCODE, self.return_type, packet, count)?;
            return self.write_return(&result);
        }
        if module.id == 3
            && let Some(operation) = ["kind", "children", "opaque_name", "resolve_raw", "fields", "variants"]
                .iter().position(|name| *name == declaration.name)
        {
            if arguments.len() != 1 || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Type {
                return Err("native type reflection ABI signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let result = self.object(node, helpers::REFLECT, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        if module.id == 19
            && matches!(
                declaration.name.as_str(),
                "compile" | "is_match" | "prepare"
            )
        {
            let regex = |ty: TypeKey| matches!(self.mir.types[ty.index()].constructor, TypeConstructor::Native(native) if (native.module, native.slot) == (19, 0));
            let kind = |ty: TypeKey| &self.mir.types[ty.index()].constructor;
            let (operation, valid) = match declaration.name.as_str() {
                "compile" => (
                    0,
                    arguments.len() == 1
                        && kind(arguments[0]) == &TypeConstructor::String
                        && regex(self.return_type),
                ),
                "is_match" => (
                    1,
                    arguments.len() == 2
                        && regex(arguments[0])
                        && kind(arguments[1]) == &TypeConstructor::String
                        && kind(self.return_type) == &TypeConstructor::Bool,
                ),
                _ => (
                    2,
                    arguments.len() == 3
                        && regex(arguments[0])
                        && kind(arguments[1]) == &TypeConstructor::Type
                        && kind(arguments[2]) == &TypeConstructor::Type
                        && regex(self.return_type),
                ),
            };
            if !valid {
                return Err("native Regex ABI signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, operation);
            let result = self.object(node, helpers::REGEX, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        if module.id == 33 && let Some(operation) = ["should_ok", "should_fail", "should_fail_with", "with_fixtures"].iter().position(|name| *name == declaration.name) {
            if arguments.len() != if operation < 2 { 1 } else { 2 }
                || !matches!(self.mir.types[self.return_type.index()].constructor, TypeConstructor::Native(id) if (id.module, id.slot) == (33, 0)) { return Err("native test result/arity mismatch".into()); }
            let callback = &self.mir.types[arguments[usize::from(operation == 3)].index()];
            if callback.constructor != TypeConstructor::Function || callback.arguments.len() != if operation == 3 { 2 } else { 1 } { return Err("native test callback signature mismatch".into()); }
            if operation == 2 && self.mir.types[arguments[1].index()].constructor != TypeConstructor::String { return Err("native test expectation must be String".into()); }
            if operation == 3 {
                let sources = &self.mir.types[arguments[0].index()];
                if sources.constructor != TypeConstructor::Array || sources.arguments.len() != 1
                    || self.mir.types[sources.arguments[0].index()].constructor != TypeConstructor::String
                    || TypeKey::try_from(callback.arguments[1])? != self.return_type { return Err("native fixture signature mismatch".into()); }
            }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let value = self.object(node, helpers::MAKE_TEST, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 1 && declaration.name == "equal" {
            if arguments.len() != 2 || arguments[0] != arguments[1] || self.mir.types[self.return_type.index()].constructor != TypeConstructor::Bool { return Err("native equality signature mismatch".into()); }
            let zero = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::EQUAL, self.return_type, data, zero)?;
            return self.write_return(&value);
        }
        if module.id == 26 && declaration.name == "call_with_diagnostics" {
            if arguments.len() != 6 { return Err("diagnostic scope arity mismatch".into()); }
            let callback = &self.mir.types[arguments[0].index()];
            let result = &self.mir.types[self.return_type.index()];
            if callback.constructor != TypeConstructor::Function || callback.arguments.len() != 2
                || callback.arguments[0].index() != arguments[1].index()
                || result.constructor != TypeConstructor::Result || result.arguments.len() != 2
            { return Err("diagnostic scope signature mismatch".into()); }
            let success = &self.mir.types[result.arguments[0].index()];
            let reports = &self.mir.types[result.arguments[1].index()];
            if success.constructor != TypeConstructor::Tuple || success.arguments != [callback.arguments[1], result.arguments[1]]
                || reports.constructor != TypeConstructor::Array || reports.arguments.len() != 1
                || arguments[2..].iter().any(|ty| self.mir.types[ty.index()].constructor != TypeConstructor::TypeOf)
                || self.mir.types[arguments[2].index()].arguments != reports.arguments
            { return Err("diagnostic scope result mismatch".into()); }
            let dispatcher = self.functions.dispatcher(arguments[0], self.module)?;
            let dispatcher = self.module.declare_func_in_func(dispatcher, self.builder.func);
            let address = self.builder.ins().func_addr(self.module.target_config().pointer_type(), dispatcher);
            let mut packet = vec![address];
            let width = arguments.iter().map(|&ty| self.layouts.words(ty)).collect::<Result<Vec<_>>>()?.into_iter().sum::<usize>();
            for index in 0..width { packet.push(self.builder.ins().load(types::I64, MemFlagsData::new(), data, (index * 8) as i32)); }
            let packet = self.stack_words(&packet)?;
            let zero = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::DIAGNOSTIC_SCOPE, self.return_type, packet, zero)?;
            return self.write_return(&value);
        }
        if module.id == 16 && let Some(operation) = ["sha256", "new", "update_bytes", "update_string", "update_int", "finish"].iter().position(|name| *name == declaration.name) {
            let hash = |ty: TypeKey| matches!(self.mir.types[ty.index()].constructor, TypeConstructor::Native(id) if (id.module, id.slot) == (16, 3));
            let kind = |ty: TypeKey| &self.mir.types[ty.index()].constructor;
            let valid = match operation {
                0 => arguments.len() == 1 && kind(arguments[0]) == &TypeConstructor::String && kind(self.return_type) == &TypeConstructor::String,
                1 => arguments.is_empty() && hash(self.return_type),
                2..=4 => arguments.len() == 2 && hash(arguments[0]) && hash(self.return_type)
                    && kind(arguments[1]) == &[TypeConstructor::Bytes, TypeConstructor::String, TypeConstructor::Int][operation - 2],
                _ => arguments.len() == 1 && hash(arguments[0]) && kind(self.return_type) == &TypeConstructor::Bytes,
            };
            if !valid { return Err("native hash signature mismatch".into()); }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let value = self.object(node, helpers::HASH, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 8 && let Some(operation) = ["join", "normalize", "parent", "file_name"].iter().position(|name| *name == declaration.name) {
            if arguments.len() != 1 { return Err("native path arity mismatch".into()); }
            let input = &self.mir.types[arguments[0].index()];
            let output = &self.mir.types[self.return_type.index()];
            let input_valid = if operation == 0 {
                input.constructor == TypeConstructor::Array && input.arguments.len() == 1
                    && self.mir.types[input.arguments[0].index()].constructor == TypeConstructor::String
            } else { input.constructor == TypeConstructor::String };
            let output_valid = if operation < 2 { output.constructor == TypeConstructor::String }
                else { output.constructor == TypeConstructor::Option && output.arguments.len() == 1
                    && self.mir.types[output.arguments[0].index()].constructor == TypeConstructor::String };
            if !input_valid || !output_valid { return Err("native path signature mismatch".into()); }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let value = self.object(node, helpers::PATH, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 17 && declaration.name == "schema_with" {
            if arguments.len() != 3
                || self.mir.types[arguments[1].index()].constructor != TypeConstructor::Type
                || self.mir.types[arguments[2].index()].constructor != TypeConstructor::TypeOf
                || self.mir.types[arguments[2].index()].arguments.len() != 1
                || TypeKey::try_from(self.mir.types[arguments[2].index()].arguments[0])? != self.return_type
            { return Err("JSON schema signature mismatch".into()); }
            let roots = (0..self.mir.types.len()).collect();
            let (packet, count) = self.codec_packet_roots(data, roots, false)?;
            let value = self.object(node, helpers::JSON_SCHEMA, self.return_type, packet, count)?;
            return self.write_return(&value);
        }
        if module.id == 17 && declaration.name == "stringify_pretty" {
            let factory = &self.mir.types[known(self.mir, node)?.index()];
            if factory.arguments.len() != 2 { return Err("JSON pretty factory ABI mismatch".into()); }
            let int = TypeKey::try_from(factory.arguments[0])?;
            let configured = TypeKey::try_from(factory.arguments[1])?;
            let signature = &self.mir.types[configured.index()];
            if self.mir.types[int.index()].constructor != TypeConstructor::Int
                || signature.constructor != TypeConstructor::Function || signature.arguments.len() != 2
                || self.mir.types[signature.arguments[1].index()].constructor != TypeConstructor::String
            { return Err("JSON pretty signature mismatch".into()); }
            let zero = self.builder.ins().iconst(types::I64, 0);
            if !self.function_key.configured_factory {
                if arguments != [int] || self.return_type != configured { return Err("JSON pretty factory arguments mismatch".into()); }
                let captured = self.object(node, helpers::JSON_INDENT, int, data, zero)?;
                let function = self.functions.declare(self.mir, functions::Key { configured_factory: true, ..self.function_key }, self.module)?;
                let mut words = vec![self.builder.ins().iconst(types::I64, i64::from(function.as_u32()))];
                words.extend(captured);
                let count = self.builder.ins().iconst(types::I64, words.len() as i64);
                let packet = self.stack_words(&words)?;
                let result = self.object(node, helpers::CLOSURE, configured, packet, count)?;
                return self.write_return(&result);
            }
            let captured = self.object(node, helpers::CAPTURE, int, environment, zero)?;
            let one = self.builder.ins().iconst(types::I64, 1);
            let count = self.builder.ins().iadd(captured[2], one);
            let result = self.object(node, helpers::JSON_STRINGIFY, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        if module.id == 17 && declaration.name == "stringify" {
            if arguments.len() != 1 || self.mir.types[self.return_type.index()].constructor != TypeConstructor::String { return Err("native JSON stringify signature mismatch".into()); }
            let count = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::JSON_STRINGIFY, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if matches!(module.id, 17 | 24 | 9) && declaration.name == "parse_raw" {
            let output = &self.mir.types[self.return_type.index()];
            if arguments.len() != 2
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::TypeOf
                || self.mir.types[arguments[0].index()].arguments.len() != 1
                || self.mir.types[arguments[1].index()].constructor != TypeConstructor::String
                || output.constructor != TypeConstructor::Result || output.arguments.len() != 2
                || output.arguments[0] != self.mir.types[arguments[0].index()].arguments[0]
                || !matches!(self.mir.types[output.arguments[1].index()].constructor, TypeConstructor::Native(id) if (id.module, id.slot) == (34, 0))
            { return Err("native data parser signature mismatch".into()); }
            let format = match module.id { 17 => 0, 24 => 1, _ => 2 };
            let count = self.builder.ins().iconst(types::I64, format);
            let value = self.object(node, helpers::FORMAT_PARSE, self.return_type, data, count)?;
            return self.write_return(&value);
        }
        if module.id == 7 && declaration.name == "parse_with" {
            let output = &self.mir.types[self.return_type.index()];
            if arguments.len() != 3
                || self.mir.types[arguments[0].index()].constructor != TypeConstructor::Type
                || self.mir.types[arguments[1].index()].constructor != TypeConstructor::TypeOf
                || self.mir.types[arguments[1].index()].arguments.len() != 1
                || self.mir.types[arguments[2].index()].constructor != TypeConstructor::String
                || output.constructor != TypeConstructor::Result || output.arguments.len() != 2
                || output.arguments[0] != self.mir.types[arguments[1].index()].arguments[0]
                || self.mir.types[output.arguments[1].index()].constructor != TypeConstructor::String
            { return Err("native string parse signature mismatch".into()); }
            let (packet, count) = self.codec_packet(data, TypeKey::try_from(output.arguments[0])?, true)?;
            let value = self.object(node, helpers::PARSE, self.return_type, packet, count)?;
            return self.write_return(&value);
        }
        if module.id == 7
            && let Some(operation) = [
                "join",
                "join_lines",
                "split",
                "lines",
                "starts_with",
                "ends_with",
                "contains",
                "replace",
                "indent",
                "ensure_trailing_newline",
                "trim_margin",
            ]
            .iter()
            .position(|&name| name == declaration.name)
        {
            let arity = match operation {
                1 | 3 | 9 => 1,
                7 => 3,
                _ => 2,
            };
            let array_string = |ty: TypeKey| {
                let shape = &self.mir.types[ty.index()];
                shape.constructor == TypeConstructor::Array
                    && shape.arguments.len() == 1
                    && self.mir.types[shape.arguments[0].index()].constructor
                        == TypeConstructor::String
            };
            let inputs_valid = arguments.len() == arity
                && arguments.iter().enumerate().all(|(index, &ty)| {
                    if operation <= 1 && index == 0 {
                        array_string(ty)
                    } else {
                        self.mir.types[ty.index()].constructor
                            == if operation == 8 && index == 1 {
                                TypeConstructor::Int
                            } else {
                                TypeConstructor::String
                            }
                    }
                });
            let output_valid = if operation == 2 || operation == 3 {
                array_string(self.return_type)
            } else {
                self.mir.types[self.return_type.index()].constructor
                    == if (4..=6).contains(&operation) {
                        TypeConstructor::Bool
                    } else {
                        TypeConstructor::String
                    }
            };
            if !inputs_valid || !output_valid {
                return Err("native String ABI signature mismatch".into());
            }
            let count = self.builder.ins().iconst(types::I64, operation as i64);
            let value = self.object(node, helpers::TEXT_OP, self.return_type, data, count)?;
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
        if matches!((module.id, declaration.name.as_str()), (5 | 6, "fold") | (5, "fold_control")) {
            let controlled = declaration.name == "fold_control";
            if arguments.len() != 3 { return Err("native fold signature mismatch".into()); }
            let output = &self.mir.types[self.return_type.index()];
            if if controlled { output.constructor != TypeConstructor::FoldControl || output.arguments.len() != 2 || output.arguments[0].index() != arguments[1].index() } else { arguments[1] != self.return_type } {
                return Err("native fold accumulator/result signature mismatch".into());
            }
            let dictionary = module.id == 6;
            let container = &self.mir.types[arguments[0].index()];
            let callback = &self.mir.types[arguments[2].index()];
            let expected = if dictionary { TypeConstructor::Dict } else { TypeConstructor::Array };
            if container.constructor != expected || container.arguments.len() != 1 || callback.constructor != TypeConstructor::Function || callback.arguments.len() != if dictionary { 4 } else { 3 }
                || callback.arguments[0].index() != arguments[1].index()
                || callback.arguments.last().unwrap().index() != self.return_type.index()
                || callback.arguments[if dictionary { 2 } else { 1 }] != container.arguments[0]
                || (dictionary && self.mir.types[callback.arguments[1].index()].constructor != TypeConstructor::String)
            { return Err("native fold callback does not match its sealed signature".into()); }
            let dispatcher = self.functions.dispatcher(arguments[2], self.module)?;
            let dispatcher = self.module.declare_func_in_func(dispatcher, self.builder.func);
            let address = self.builder.ins().func_addr(self.module.target_config().pointer_type(), dispatcher);
            let mut packet = vec![address];
            let width = arguments.iter().map(|&ty| self.layouts.words(ty)).collect::<Result<Vec<_>>>()?.into_iter().sum::<usize>();
            for i in 0..width { packet.push(self.builder.ins().load(types::I64, MemFlagsData::new(), data, (i * 8) as i32)); }
            let data = self.stack_words(&packet)?;
            let count = self.builder.ins().iconst(types::I64, if controlled { 2 } else { i64::from(dictionary) });
            let result = self.object(node, helpers::FOLD, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        if matches!((module.id, declaration.name.as_str()), (5, "map" | "flat_map" | "find" | "filter" | "any" | "all") | (6, "map_values" | "filter")) {
            let dictionary = module.id == 6;
            let find = declaration.name == "find";
            let filter = declaration.name == "filter";
            let boolean = matches!(declaration.name.as_str(), "any" | "all");
            let predicate = find || filter || boolean;
            let constructor = if dictionary { TypeConstructor::Dict } else { TypeConstructor::Array };
            if arguments.len() != 2 {
                return Err("native map arity mismatch".into());
            }
            let array = &self.mir.types[arguments[0].index()];
            let callback = &self.mir.types[arguments[1].index()];
            let output = &self.mir.types[self.return_type.index()];
            if array.constructor != constructor
                || output.constructor != if find { TypeConstructor::Option } else if boolean { TypeConstructor::Bool } else { constructor }
                || callback.constructor != TypeConstructor::Function
                || callback.arguments.len() != 2
                || array.arguments != callback.arguments[..1]
                || if predicate {
                    (!boolean && output.arguments != array.arguments) || self.mir.types[callback.arguments[1].index()].constructor != TypeConstructor::Bool
                } else if declaration.name == "flat_map" { callback.arguments[1].index() != self.return_type.index() }
                else { output.arguments != callback.arguments[1..] }
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
            let operation = match declaration.name.as_str() { "find" => 2, "filter" if dictionary => 6, "filter" => 3, "any" => 4, "all" => 5, "flat_map" => 7, _ => i64::from(dictionary) };
            let count = self.builder.ins().iconst(types::I64, operation);
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
