//! Schema entry dispatches on closed TypeIds; helpers never infer types.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Plan, Special},
};
use telora_core::mir::{SealedExecutable, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Plan {
    pub(crate) fn plan_schemas(&mut self, executable: &SealedExecutable<'_>) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        for root in executable.closure().nodes() {
            if crate::natives::identity(mir, root.node) != Some((17, "schema_with")) {
                continue;
            }
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let args = &mir.types[key.ty(mir, root.node)?.index()].arguments;
            if args.len() != 4
                || mir.types[args[2].index()].constructor != T::TypeOf
                || mir.types[args[2].index()].arguments != [args[3]]
            {
                return Err("Wasm: schema signature mismatch".into());
            }
            for layout in &self.layouts {
                self.functions.insert(
                    Key {
                        special: Special::Schema(layout.id(), args[3]),
                        callable: true,
                        ..self.root
                    },
                    0,
                );
            }
        }
        Ok(())
    }
}

impl Emitter<'_> {
    pub(crate) fn schema_native(&mut self) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let metadata = self.parameter(1);
        let properties = self.parameter(0);
        let properties = self.codec_property_context(args[0], properties)?;
        let context = self.alloc(36);
        self.copy(context, 0, properties, 24);
        let links = self.alloc(self.plan.layouts.len() as u32 * 4);
        let definitions = self.alloc(self.plan.layouts.len() as u32 * 4);
        self.extend([
            I::LocalGet(links),
            I::I32Const(0),
            I::I32Const(self.plan.layouts.len() as i32 * 4),
            I::MemoryFill(0),
        ]);
        for (offset, value) in [(24, links), (28, definitions)] {
            self.extend([
                I::LocalGet(context),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.store32(context, 32, 0);
        let id = self.read32(metadata, DATA);
        let result = self.local(ValType::I32);
        self.emit(I::Block(BlockType::Empty));
        for layout in &self.plan.layouts {
            self.extend([
                I::LocalGet(id),
                I::I32Const(layout.type_id as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let value = self.schema_call(layout.id(), args[3], context, metadata)?;
            self.extend([I::LocalGet(value), I::LocalSet(result), I::Br(1), I::End]);
        }
        self.emit(I::End);
        self.extend([I::LocalGet(result), I::I32Eqz, I::If(BlockType::Empty)]);
        self.codec_error(metadata, "JSON Schema requires a sealed TypeId")?;
        self.emit(I::End);
        self.schema_definitions(args[3], result, context, metadata)?;
        let dialect = self.schema_string(
            args[3],
            b"https://json-schema.org/draft/2020-12/schema",
            metadata,
        )?;
        self.schema_add_field(args[3], result, "$schema", dialect, metadata)
    }

    pub(crate) fn schema_call(
        &mut self,
        source: TypeId,
        target: TypeId,
        context: u32,
        input: u32,
    ) -> Result<u32, String> {
        let key = Key {
            special: Special::Schema(source, target),
            callable: true,
            ..self.plan.root
        };
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: schema type was not planned")?;
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(context),
            I::LocalGet(input),
            I::Call(function),
            I::LocalSet(result),
        ]);
        self.checked(result);
        Ok(result)
    }

    pub(crate) fn schema_type(&mut self, source: TypeId, target: TypeId) -> Result<u32, String> {
        let ty = &self.mir.types[source.index()];
        let args = ty.arguments.clone();
        match &ty.constructor {
            T::Nominal(_) => self.schema_nominal(source, target),
            T::Record(names) => self.schema_record(target, names.clone(), args, false),
            T::Enum(_) | T::Result | T::FoldControl | T::PropertyTarget => {
                self.schema_enum(source, target, false, false)
            }
            T::Int | T::Float | T::String | T::Bool => {
                let name = match ty.constructor {
                    T::Int => "integer",
                    T::Float => "number",
                    T::String => "string",
                    _ => "boolean",
                };
                self.schema_kind(target, name)
            }
            T::Array | T::Dict => {
                let array = ty.constructor == T::Array;
                let kind =
                    self.schema_string(target, if array { b"array" } else { b"object" }, 1)?;
                let child = self.schema_call(args[0], target, 0, 1)?;
                self.schema_object(
                    target,
                    vec![
                        ("type".into(), kind),
                        (
                            if array {
                                "items"
                            } else {
                                "additionalProperties"
                            }
                            .into(),
                            child,
                        ),
                    ],
                    1,
                )
            }
            T::Option => {
                let null = self.schema_kind(target, "null")?;
                let child = self.schema_call(args[0], target, 0, 1)?;
                let array = self.schema_array(target, &[null, child], 1)?;
                self.schema_object(target, vec![("anyOf".into(), array)], 1)
            }
            T::Tuple => {
                let mut items = Vec::new();
                for ty in args.iter().copied() {
                    items.push(self.schema_call(ty, target, 0, 1)?);
                }
                let items = self.schema_array(target, &items, 1)?;
                let kind = self.schema_string(target, b"array", 1)?;
                let len = self.schema_integer(target, args.len() as i64, 1)?;
                self.schema_object(
                    target,
                    vec![
                        ("type".into(), kind),
                        ("prefixItems".into(), items),
                        ("minItems".into(), len),
                        ("maxItems".into(), len),
                    ],
                    1,
                )
            }
            T::Unchecked | T::Newtype => self.schema_call(args[0], target, 0, 1),
            _ => {
                let message = match ty.constructor {
                    T::Type | T::TypeOf => "JSON Schema cannot describe Type metadata".into(),
                    T::Dyn => "JSON Schema cannot describe Dyn".into(),
                    T::Bytes => "Type Bytes has no JSON Schema mapping".into(),
                    T::Function => "Type Func has no JSON Schema mapping".into(),
                    T::Native(_) => "Type Opaque has no JSON Schema mapping".into(),
                    T::Parameter(_) => "JSON Schema requires a concrete type".into(),
                    _ => format!("Type {:?} has no JSON Schema mapping", ty.constructor),
                };
                self.codec_error(1, &message)?;
                Ok(self.local(ValType::I32))
            }
        }
    }
    pub(crate) fn schema_kind(&mut self, target: TypeId, name: &str) -> Result<u32, String> {
        let name = self.schema_string(target, name.as_bytes(), 1)?;
        self.schema_object(target, vec![("type".into(), name)], 1)
    }
}
