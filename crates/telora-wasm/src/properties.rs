use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{
    HirId, HirKind, PropertyAdmission, PropertySite, Role, TypeConstructor as T, TypeId,
};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn property_chain(&mut self, index: usize) -> Result<u32, String> {
        let record = &self.mir.properties[index];
        let node = record.providers[0];
        if let Some(PropertyAdmission::Require {
            capability,
            targets,
        }) = record.admission
        {
            let key = *self
                .plan
                .properties
                .get(&capability.index())
                .ok_or("Wasm: missing admitted property capability")?;
            let capability = self.call_key(key)?;
            let contents = self.table_data(RECORDS, capability, DATA);
            self.extend([
                I::LocalGet(contents),
                I::I64Load(memory(DATA, 3)),
                I::I64Const(targets),
                I::I64And,
                I::I64Eqz,
            ]);
            self.fail_if(node, ERROR_PROPERTY);
        }
        let mut previous = None;
        for &provider in &record.providers {
            let callee = child(self.mir, provider, Role::Callee)?;
            let mut signature = self.ty(callee)?;
            let mut closure = self.expression(callee)?;
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
                let expected = &self.mir.types[signature.index()].arguments;
                if expected.len() != arguments.len() + 1 {
                    return Err(
                        "Wasm: configured property arity differs from sealed signature".into(),
                    );
                }
                let mut values = vec![];
                for (&argument, &target) in arguments.iter().zip(expected) {
                    let value = self.expression(argument)?;
                    values.push(self.adapt(
                        argument,
                        self.effective_ty(argument)?,
                        target,
                        value,
                    )?);
                }
                closure = self.invoke(closure, &values)?;
                signature = *expected.last().unwrap();
            }
            let shape = &self.mir.types[signature.index()];
            if shape.constructor != T::Function
                || shape.arguments.len() != 3
                || shape.arguments[2] != record.property
            {
                return Err("Wasm: property provider signature mismatch".into());
            }
            let owner_ty = shape.arguments[0];
            let optional = shape.arguments[1];
            if self.mir.types[optional.index()].constructor != T::Option
                || self.mir.types[optional.index()].arguments != [record.property]
            {
                return Err("Wasm: property previous-value signature mismatch".into());
            }
            let owner = self.property_context(provider, index, owner_ty)?;
            let prior =
                self.enum_value(provider, optional, u32::from(previous.is_some()), previous)?;
            previous = Some(self.invoke(closure, &[owner, prior])?);
        }
        previous.ok_or_else(|| "Wasm: empty property chain".into())
    }
    fn property_context(&mut self, node: HirId, index: usize, ty: TypeId) -> Result<u32, String> {
        let record = &self.mir.properties[index];
        if record.site == PropertySite::Type {
            if !matches!(self.mir.types[ty.index()].constructor, T::Type | T::TypeOf) {
                return Err("Wasm: property owner is not metadata".into());
            }
            return self.scalar_as(node, ty, record.owner.index() as i64);
        }
        let member_index = match record.site {
            PropertySite::Field(index) | PropertySite::Variant(index) => index as usize,
            _ => unreachable!(),
        };
        let T::Nominal(symbol) = self.mir.types[record.owner.index()].constructor else {
            return Err("Wasm: property member owner is not nominal".into());
        };
        let declaration = self
            .mir
            .type_definitions
            .iter()
            .find(|d| d.symbol == symbol)
            .ok_or("Wasm: member owner has no definition")?;
        let name = &declaration
            .members
            .get(member_index)
            .ok_or("Wasm: property member index is invalid")?
            .name;
        let payload = self.mir.type_layouts[record.owner.index()]
            .as_ref()
            .and_then(|l| l.members.get(member_index))
            .copied()
            .flatten();
        let object = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: property context has no layout")?;
        let bytes = object
            .bytes
            .ok_or("Wasm: property context has no fixed size")? as u32;
        let fields = object
            .members
            .iter()
            .map(|f| {
                Ok((
                    f.name.clone(),
                    self.plan.layouts[f.type_id.ok_or("Wasm: context field has no type")?].id(),
                    f.offset.ok_or("Wasm: context field offset missing")? as u32,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let data = self.alloc(bytes);
        for (field, field_ty, offset) in fields {
            let value = match field.as_str() {
                "owner" => self.scalar_as(node, field_ty, record.owner.index() as i64)?,
                "index" => self.scalar_as(node, field_ty, member_index as i64)?,
                "name" => self.text_as(node, field_ty, name.as_bytes())?,
                "ty" => self.scalar_as(
                    node,
                    field_ty,
                    payload.ok_or("Wasm: field has no payload type")?.index() as i64,
                )?,
                "payload" => {
                    let metadata = *self.mir.types[field_ty.index()]
                        .arguments
                        .first()
                        .ok_or("Wasm: variant context payload has no option type")?;
                    let value = payload
                        .map(|ty| self.scalar_as(node, metadata, ty.index() as i64))
                        .transpose()?;
                    self.enum_value(node, field_ty, u32::from(value.is_some()), value)?
                }
                _ => return Err("Wasm: property context differs from admitted ABI".into()),
            };
            self.copy(data, offset, value, self.width(field_ty)?);
        }
        let id = self.table_push(RECORDS, data, bytes);
        let result = self.value_as(node, ty, self.width(ty)?)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
    pub fn property_query(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let signature = self.ty(node)?;
        let args = &self.mir.types[signature.index()].arguments;
        let evidence = name == "evidence";
        let member = matches!(name, "get_field_prop" | "get_variant_prop");
        let property_argument = if member { 2 } else { 1 };
        if args.len() != property_argument + 2
            || self.mir.types[args[property_argument].index()].constructor != T::TypeOf
        {
            return Err("Wasm: property query ABI signature mismatch".into());
        }
        let property = self.mir.types[args[property_argument].index()].arguments[0];
        let output = *args.last().unwrap();
        if (evidence && output != property)
            || (!evidence
                && (self.mir.types[output.index()].constructor != T::Option
                    || self.mir.types[output.index()].arguments != [property]))
        {
            return Err("Wasm: property query result differs from its witness".into());
        }
        let owner = self.parameter(0);
        let index = member.then(|| self.parameter(1));
        let result = self.local(ValType::I32);
        self.emit(I::Block(BlockType::Empty));
        for (&property_index, &key) in &self.plan.properties {
            let record = &self.mir.properties[property_index];
            if record.property != property {
                continue;
            }
            let target_index = match (name, record.site) {
                ("get_type_prop" | "evidence", PropertySite::Type) => None,
                ("get_field_prop", PropertySite::Field(index))
                | ("get_variant_prop", PropertySite::Variant(index)) => Some(index),
                _ => continue,
            };
            self.bits(owner);
            self.extend([I::I64Const(record.owner.index() as i64), I::I64Eq]);
            if let Some(member_index) = target_index {
                self.bits(index.unwrap());
                self.extend([I::I64Const(member_index as i64), I::I64Eq, I::I32And]);
            }
            self.emit(I::If(BlockType::Empty));
            let value = self.call_key(key)?;
            let value = if evidence {
                value
            } else {
                self.enum_value(node, output, 1, Some(value))?
            };
            self.extend([I::LocalGet(value), I::LocalSet(result), I::Br(1), I::End]);
        }
        if evidence {
            self.failure(node, ERROR_PROPERTY);
        } else {
            let absent = self.enum_value(node, output, 0, None)?;
            self.extend([I::LocalGet(absent), I::LocalSet(result)]);
        }
        self.emit(I::End);
        Ok(result)
    }
}
