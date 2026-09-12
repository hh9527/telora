use super::*;
use cranelift_module::FuncId;
use telora_core::mir::{PropertyAdmission, PropertySite};

impl functions::Functions {
    pub(super) fn property(
        &mut self,
        graph: &Mir,
        index: usize,
        module: &mut JITModule,
    ) -> Result<(u32, FuncId)> {
        if let Some(&entry) = self.properties.get(&index) {
            return Ok(entry);
        }
        let record = graph
            .properties
            .get(index)
            .ok_or("property outside sealed graph")?;
        if !record.concrete || record.providers.is_empty() {
            return Err("property has no concrete provider chain".into());
        }
        let slot = u32::try_from(self.globals.len() + self.instances.len() + self.properties.len() + self.checks.len())
            .map_err(|_| "native demand index overflow")?;
        let function = module
            .declare_function(
                &format!("telora_property_{index}"),
                Linkage::Local,
                &self.signature,
            )
            .map_err(|e| e.to_string())?;
        self.properties.insert(index, (slot, function));
        Ok((slot, function))
    }
}

pub(super) fn emit(
    graph: &Mir,
    layouts: &Layouts,
    index: usize,
    helper: FuncId,
    module: &mut JITModule,
    functions: &mut functions::Functions,
) -> Result<()> {
    let record = &graph.properties[index];
    let node = record.providers[0];
    let function = functions.properties[&index].1;
    let mut ctx = module.make_context();
    ctx.func.signature = functions.signature.clone();
    let object_helper = module.declare_func_in_func(helper, &mut ctx.func);
    let mut fbctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);
    let context = builder.block_params(entry)[0];
    let out = builder.block_params(entry)[2];
    let mut lower = Lower {
        local_instances: BTreeMap::new(),
        guarded: false, frame_charge: None,
        mir: graph,
        layouts,
        builder,
        locals: BTreeMap::new(),
        module,
        context,
        object_helper,
        functions,
        function_key: functions::Key {
            node,
            instance: record.instance,
            initializer: false,
            configured_factory: false,
        },
        return_pointer: out,
        return_type: TypeKey::try_from(record.property)?,
    };
    lower.enter_call(node);
    match lower.property_chain(index) {
        Ok(()) | Err(EmitError::Diverged) => {}
        Err(EmitError::Message(message)) => return Err(message),
    }
    let config = lower.module.target_config();
    lower.seal_frame_charge();
        lower.builder.finalize(config);
    module
        .define_function(function, &mut ctx)
        .map_err(|e| e.to_string())
}

impl Lower<'_, '_> {
    pub(super) fn codec_packet(&mut self, data: ir::Value, root: TypeKey, decode: bool) -> EmitResult<(ir::Value, ir::Value)> {
        self.codec_packet_roots(data, vec![root.index()], decode)
    }
    pub(super) fn codec_packet_roots(&mut self, data: ir::Value, pending: Vec<usize>, decode: bool) -> EmitResult<(ir::Value, ir::Value)> {
        self.codec_packet_impl(data, pending, decode, true)
    }
    pub(super) fn cast_packet(&mut self, data: ir::Value, target: TypeKey) -> EmitResult<(ir::Value, ir::Value)> {
        self.codec_packet_impl(data, vec![target.index()], true, false)
    }
    fn codec_packet_impl(&mut self, data: ir::Value, mut pending: Vec<usize>, decode: bool, include_properties: bool) -> EmitResult<(ir::Value, ir::Value)> {
        let mut reachable = std::collections::BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !reachable.insert(ty) { continue; }
            pending.extend(self.mir.types[ty].arguments.iter().map(|ty| ty.index()));
            if let Some(layout) = &self.mir.type_layouts[ty] {
                pending.extend(layout.members.iter().flatten().map(|ty| ty.index()));
            }
        }
        let checks = self.mir.construction_checks.iter().enumerate()
            .filter(|(_, check)| decode && check.concrete && reachable.contains(&check.owner.index()))
            .map(|(index, check)| (index, check.owner, check.site)).collect::<Vec<_>>();
        let properties = self.mir.properties.iter().enumerate()
            .filter(|(_, property)| include_properties && property.concrete && property.site == PropertySite::Type && reachable.contains(&property.owner.index()))
            .map(|(index, property)| (index, property.owner, property.property)).collect::<Vec<_>>();
        let mut packet = vec![data];
        for &(index, owner, site) in &checks {
            let (slot, initializer, signature) = self.functions.check(self.mir, index, self.module)?;
            let dispatcher = self.functions.dispatcher(signature, self.module)?;
            let site = match site { PropertySite::Type => 0, PropertySite::Variant(index) => u64::from(index) + 1, _ => return Err("unsupported checker site".into()) };
            for word in [owner.index() as u64, site, u64::from(slot), u64::from(signature.raw())] {
                packet.push(self.builder.ins().iconst(types::I64, word as i64));
            }
            for function in [initializer, dispatcher] {
                let reference = self.module.declare_func_in_func(function, self.builder.func);
                packet.push(self.builder.ins().func_addr(self.module.target_config().pointer_type(), reference));
            }
        }
        for &(index, owner, property) in &properties {
            let (slot, initializer) = self.functions.property(self.mir, index, self.module)?;
            for word in [owner.index() as u64, property.index() as u64, u64::from(slot)] {
                packet.push(self.builder.ins().iconst(types::I64, word as i64));
            }
            let reference = self.module.declare_func_in_func(initializer, self.builder.func);
            packet.push(self.builder.ins().func_addr(self.module.target_config().pointer_type(), reference));
            let display = self.layouts.field_names[property.index()].iter().position(|name| name == "display")
                .and_then(|index| self.mir.type_layouts[property.index()].as_ref()?.members.get(index).copied().flatten())
                .filter(|signature| self.mir.types[signature.index()].constructor == TypeConstructor::Function);
            let address = if let Some(signature) = display {
                let dispatcher = self.functions.dispatcher(TypeKey::try_from(signature)?, self.module)?;
                let reference = self.module.declare_func_in_func(dispatcher, self.builder.func);
                self.builder.ins().func_addr(self.module.target_config().pointer_type(), reference)
            } else { self.builder.ins().iconst(types::I64, 0) };
            packet.push(address);
        }
        let counts = u64::from(u32::try_from(checks.len()).map_err(|_| "codec check count overflow")?)
            | (u64::from(u32::try_from(properties.len()).map_err(|_| "codec property count overflow")?) << 32);
        let packet = self.stack_words(&packet)?;
        let count = self.builder.ins().iconst(types::I64, counts as i64);
        Ok((packet, count))
    }
    pub(super) fn construction_check(&mut self, node: HirId, owner: TypeKey, site: PropertySite, value: &[ir::Value]) -> EmitResult<()> {
        let checks = self.mir.construction_checks.iter().enumerate()
            .filter(|(_, check)| check.concrete && check.owner.index() == owner.index() && check.site == site)
            .map(|(index, _)| index).collect::<Vec<_>>();
        for index in checks {
            let (slot, initializer, signature) = self.functions.check(self.mir, index, self.module)?;
            let closure = self.demand(node, slot, initializer, signature)?;
            let shape = &self.mir.types[signature.index()];
            if shape.constructor != TypeConstructor::Function || shape.arguments.len() != 2 { return Err("checker has no sealed unary signature".into()); }
            let argument = TypeKey::try_from(shape.arguments[0])?;
            let output = TypeKey::try_from(shape.arguments[1])?;
            let mut value = value.to_vec();
            if self.mir.types[argument.index()].constructor == TypeConstructor::Unchecked {
                if self.mir.types[argument.index()].arguments != [self.mir.construction_checks[index].owner] || value.len() != self.layouts.words(argument)? {
                    return Err("checker unchecked view differs from sealed owner".into());
                }
                let origin = self.builder.ins().band_imm_s(value[1], 0xffff_ffff);
                let tag = self.builder.ins().iconst(types::I64, i64::from(argument.raw()) << 32);
                value[1] = self.builder.ins().bor(origin, tag);
            }
            let result = self.invoke_provider(signature, &closure, &[value])?;
            let data = self.stack_words(&result)?;
            let count = self.builder.ins().iconst(types::I64, 0);
            self.object(node, helpers::CHECK_RESULT, output, data, count)?;
        }
        Ok(())
    }
    fn property_context(
        &mut self,
        node: HirId,
        record: &telora_core::mir::PropertyRecord,
        context: TypeKey,
    ) -> EmitResult<Vec<ir::Value>> {
        if record.site == PropertySite::Type {
            return self.fixed_value(
                node,
                context,
                &[u64::from(TypeKey::try_from(record.owner)?.raw())],
            );
        }
        let index = match record.site {
            PropertySite::Field(i) | PropertySite::Variant(i) => i as usize,
            _ => unreachable!(),
        };
        let TypeConstructor::Nominal(symbol) = self.mir.types[record.owner.index()].constructor
        else {
            return Err("member property owner has no nominal skeleton".into());
        };
        let name = self
            .mir
            .type_definitions
            .iter()
            .find(|d| d.symbol == symbol)
            .and_then(|d| d.members.get(index))
            .ok_or("property member missing from skeleton")?
            .name
            .clone();
        let payload = *self.mir.type_layouts[record.owner.index()]
            .as_ref()
            .and_then(|layout| layout.members.get(index))
            .ok_or("property member type missing from skeleton")?;
        let names = self.layouts.field_names[context.index()].clone();
        let fields = self.mir.type_layouts[context.index()]
            .as_ref()
            .ok_or("member property context has no layout")?
            .members
            .clone();
        if names.len() != 4 || fields.len() != 4 {
            return Err("member property context ABI field count mismatch".into());
        }
        let mut values = Vec::new();
        for (name_key, field) in names.iter().zip(fields) {
            let ty = TypeKey::try_from(field.ok_or("context field has no closed type")?)?;
            let value = match name_key.as_str() {
                "owner" | "ty" => {
                    if self.mir.types[ty.index()].constructor != TypeConstructor::Type {
                        return Err("context metadata ABI mismatch".into());
                    }
                    let represented = if name_key == "owner" {
                        record.owner
                    } else {
                        if !matches!(record.site, PropertySite::Field(_)) {
                            return Err("variant context cannot have a field type".into());
                        }
                        payload.ok_or("field property has no field type")?
                    };
                    self.fixed_value(
                        node,
                        ty,
                        &[u64::from(TypeKey::try_from(represented)?.raw())],
                    )?
                }
                "index" => {
                    if self.mir.types[ty.index()].constructor != TypeConstructor::Int {
                        return Err("context index ABI mismatch".into());
                    }
                    self.fixed_value(node, ty, &[index as u64])?
                }
                "name" => {
                    if self.mir.types[ty.index()].constructor != TypeConstructor::String {
                        return Err("context name ABI mismatch".into());
                    }
                    self.string(node, ty, &name)?
                }
                "payload" => {
                    if !matches!(record.site, PropertySite::Variant(_))
                        || self.mir.types[ty.index()].constructor != TypeConstructor::Option
                    {
                        return Err("context payload ABI mismatch".into());
                    }
                    let metadata = TypeKey::try_from(self.mir.types[ty.index()].arguments[0])?;
                    if self.mir.types[metadata.index()].constructor != TypeConstructor::Type {
                        return Err("variant payload metadata ABI mismatch".into());
                    }
                    if let Some(payload) = payload {
                        let value = self.fixed_value(
                            node,
                            metadata,
                            &[u64::from(TypeKey::try_from(payload)?.raw())],
                        )?;
                        self.enum_constructor(node, ty, 1, &value)?
                    } else {
                        self.enum_constructor(node, ty, 0, &[])?
                    }
                }
                _ => return Err("unknown member property context ABI field".into()),
            };
            values.extend(value);
        }
        let data = self.stack_words(&values)?;
        let count = self.builder.ins().iconst(types::I64, 4);
        self.object(node, helpers::AGGREGATE, context, data, count)
    }
    fn fixed_value(
        &mut self,
        node: HirId,
        ty: TypeKey,
        data: &[u64],
    ) -> EmitResult<Vec<ir::Value>> {
        let value = self.layouts.value(
            ty,
            Origin::from_loc(Some(self.mir.hir[node.index()].location)),
            data,
        )?;
        Ok(value
            .words()
            .iter()
            .map(|&word| self.builder.ins().iconst(types::I64, word as i64))
            .collect())
    }
    pub(super) fn invoke_provider(
        &mut self,
        signature: TypeKey,
        closure: &[ir::Value],
        values: &[Vec<ir::Value>],
    ) -> EmitResult<Vec<ir::Value>> {
        let function = &self.mir.types[signature.index()];
        if function.constructor != TypeConstructor::Function
            || function.arguments.len() != values.len() + 1
        {
            return Err("provider has no closed call signature".into());
        }
        for (value, &ty) in values.iter().zip(&function.arguments) {
            if value.len() != self.layouts.words(TypeKey::try_from(ty)?)? {
                return Err("provider argument width mismatch".into());
            }
        }
        let output = TypeKey::try_from(*function.arguments.last().unwrap())?;
        let never = self.layouts.is_never(output)?;
        let width = if never { 0 } else { self.layouts.words(output)? };
        let dispatcher = self.functions.dispatcher(signature, self.module)?;
        let dispatcher = self
            .module
            .declare_func_in_func(dispatcher, self.builder.func);
        let data = self.stack_words(&values.iter().flatten().copied().collect::<Vec<_>>())?;
        let environment = self.stack_words(closure)?;
        let zero = self.builder.ins().iconst(types::I64, 0);
        let out = self.stack_words(&vec![zero; width])?;
        let call = self
            .builder
            .ins()
            .call(dispatcher, &[self.context, data, out, environment]);
        let status = self.builder.inst_results(call)[0];
        let fail = self.builder.create_block();
        let ready = self.builder.create_block();
        self.builder.ins().brif(status, fail, &[], ready, &[]);
        self.builder.switch_to_block(fail);
        self.builder.seal_block(fail);
        self.return_status(status);
        self.builder.switch_to_block(ready);
        self.builder.seal_block(ready);
        if never {
            self.report_failure(self.function_key.node, "native Never callback returned unexpectedly")?;
            return Err(EmitError::Diverged);
        }
        Ok((0..width)
            .map(|i| {
                self.builder
                    .ins()
                    .load(types::I64, MemFlagsData::new(), out, (i * 8) as i32)
            })
            .collect())
    }
    fn property_demand(&mut self, node: HirId, index: usize) -> EmitResult<Vec<ir::Value>> {
        let ty = TypeKey::try_from(self.mir.properties[index].property)?;
        let (slot, function) = self.functions.property(self.mir, index, self.module)?;
        let function = self
            .module
            .declare_func_in_func(function, self.builder.func);
        let address = self
            .builder
            .ins()
            .func_addr(self.module.target_config().pointer_type(), function);
        let slot = self.builder.ins().iconst(types::I64, i64::from(slot));
        self.object(node, helpers::DEMAND, ty, address, slot)
    }
    fn property_chain(&mut self, index: usize) -> EmitResult<()> {
        let record = self.mir.properties[index].clone();
        let node = record.providers[0];
        if let Some(PropertyAdmission::Require {
            capability,
            targets,
        }) = record.admission
        {
            let capability_ty =
                TypeKey::try_from(self.mir.properties[capability.index()].property)?;
            let capability = self.property_demand(node, capability.index())?;
            let data = self.stack_words(&capability)?;
            let bits_index = self.layouts.field_names[capability_ty.index()]
                .iter()
                .position(|name| name == "bits")
                .ok_or("capability bits field missing")?;
            let bits_ty = self.mir.type_layouts[capability_ty.index()]
                .as_ref()
                .and_then(|l| l.members.get(bits_index))
                .copied()
                .flatten()
                .ok_or("capability bits type missing")?;
            if self.mir.types[bits_ty.index()].constructor != TypeConstructor::Int {
                return Err("capability bits ABI mismatch".into());
            }
            let field = self.builder.ins().iconst(types::I64, bits_index as i64);
            let bits = self.object(
                node,
                helpers::FIELD,
                TypeKey::try_from(bits_ty)?,
                data,
                field,
            )?;
            let accepted = self.builder.ins().band_imm_s(bits[2], targets);
            let ready = self.builder.create_block();
            let fail = self.builder.create_block();
            self.builder.ins().brif(accepted, ready, &[], fail, &[]);
            self.builder.switch_to_block(fail);
            self.builder.seal_block(fail);
            self.report_failure(node, "property type does not support this decorator target")?;
            self.builder.switch_to_block(ready);
            self.builder.seal_block(ready);
        }
        let mut previous: Option<Vec<ir::Value>> = None;
        for &provider in &record.providers {
            let callee = child(self.mir, provider, Role::Callee)?;
            let mut signature = TypeKey::try_from(self.ty(callee)?)?;
            let mut closure = self.expression(callee, 0)?;
            if matches!(
                self.mir.hir[provider.index()].kind,
                HirKind::Decorator { configured: true }
            ) {
                let arguments = self.mir.hir[provider.index()]
                    .children
                    .iter()
                    .filter(|e| e.role == Role::Argument)
                    .map(|e| e.node)
                    .collect::<Vec<_>>();
                let expected = self.mir.types[signature.index()].arguments.clone();
                let mut values = Vec::new();
                for (&arg, &ty) in arguments.iter().zip(&expected) {
                    let value = self.expression(arg, 0)?;
                    values.push(self.fit_metadata(arg, TypeKey::try_from(ty)?, value)?);
                }
                closure = self.invoke_provider(signature, &closure, &values)?;
                signature =
                    TypeKey::try_from(*expected.last().ok_or("provider factory result missing")?)?;
            }
            let shape = &self.mir.types[signature.index()];
            if shape.constructor != TypeConstructor::Function
                || shape.arguments.len() != 3
                || shape.arguments[2] != record.property
            {
                return Err("property provider signature mismatch".into());
            }
            let owner_ty = TypeKey::try_from(shape.arguments[0])?;
            let optional = TypeKey::try_from(shape.arguments[1])?;
            if record.site == PropertySite::Type
                && !matches!(
                    self.mir.types[owner_ty.index()].constructor,
                    TypeConstructor::Type | TypeConstructor::TypeOf
                )
            {
                return Err("native property owner context requires metadata".into());
            }
            if self.mir.types[owner_ty.index()].constructor == TypeConstructor::TypeOf
                && self.mir.types[owner_ty.index()].arguments != [record.owner]
            {
                return Err("property owner contradicts closed witness".into());
            }
            if self.mir.types[optional.index()].constructor != TypeConstructor::Option
                || self.mir.types[optional.index()].arguments != [record.property]
            {
                return Err("property previous value signature mismatch".into());
            }
            let owner = self.property_context(provider, &record, owner_ty)?;
            let previous_value = match &previous {
                Some(value) => self.enum_constructor(provider, optional, 1, value)?,
                None => self.enum_constructor(provider, optional, 0, &[])?,
            };
            previous = Some(self.invoke_provider(signature, &closure, &[owner, previous_value])?);
        }
        self.write_return(&previous.ok_or("property chain is empty")?)
    }
    pub(super) fn property_query(
        &mut self,
        node: HirId,
        arguments: &[TypeKey],
        data: ir::Value,
        evidence: bool,
        site: PropertySite,
    ) -> EmitResult<()> {
        let member_query = site != PropertySite::Type;
        let property_argument = if member_query { 2 } else { 1 };
        if arguments.len() != property_argument + 1
            || self.mir.types[arguments[0].index()].constructor
                != if evidence {
                    TypeConstructor::TypeOf
                } else {
                    TypeConstructor::Type
                }
            || self.mir.types[arguments[property_argument].index()].constructor
                != TypeConstructor::TypeOf
            || (member_query
                && self.mir.types[arguments[1].index()].constructor != TypeConstructor::Int)
        {
            return Err("native property query ABI mismatch".into());
        }
        let property = self.mir.types[arguments[property_argument].index()].arguments[0];
        if if evidence {
            self.return_type != TypeKey::try_from(property)?
        } else {
            self.mir.types[self.return_type.index()].constructor != TypeConstructor::Option
                || self.mir.types[self.return_type.index()].arguments != [property]
        } {
            return Err("native property query result mismatch".into());
        }
        let owner = self
            .builder
            .ins()
            .load(types::I64, MemFlagsData::new(), data, 16);
        let candidates = self
            .mir
            .properties
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.concrete
                    && std::mem::discriminant(&r.site) == std::mem::discriminant(&site)
                    && r.property == property
            })
            .map(|(i, r)| (i, r.owner, r.site))
            .collect::<Vec<_>>();
        let member = if member_query {
            Some(self.builder.ins().load(
                types::I64,
                MemFlagsData::new(),
                data,
                ((self.layouts.words(arguments[0])? + 2) * 8) as i32,
            ))
        } else {
            None
        };
        for (index, ty, record_site) in candidates {
            let mut matched = self.builder.ins().icmp_imm_s(
                cranelift_codegen::ir::condcodes::IntCC::Equal,
                owner,
                ty.index() as i64,
            );
            if let Some(member) = member {
                let position = match record_site {
                    PropertySite::Field(i) | PropertySite::Variant(i) => i,
                    PropertySite::Type => unreachable!(),
                };
                let same = self.builder.ins().icmp_imm_s(
                    cranelift_codegen::ir::condcodes::IntCC::Equal,
                    member,
                    i64::from(position),
                );
                matched = self.builder.ins().band(matched, same);
            }
            let yes = self.builder.create_block();
            let no = self.builder.create_block();
            self.builder.ins().brif(matched, yes, &[], no, &[]);
            self.builder.switch_to_block(yes);
            self.builder.seal_block(yes);
            let value = self.property_demand(node, index)?;
            let result = if evidence {
                value
            } else {
                self.enum_constructor(node, self.return_type, 1, &value)?
            };
            self.write_return(&result)?;
            self.builder.switch_to_block(no);
            self.builder.seal_block(no);
        }
        if evidence {
            return self.report_failure(
                node,
                "sealed property evidence is missing from the native plan",
            );
        }
        let result = self.enum_constructor(node, self.return_type, 0, &[])?;
        self.write_return(&result)
    }
}
