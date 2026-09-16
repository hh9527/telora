//! Codec adapters consume closed identities; no runtime type inference.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special},
};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_variant(
        &mut self,
        target: TypeId,
        name: &str,
        payload: Option<u32>,
        input: u32,
    ) -> Result<u32, String> {
        let index = self.plan.layouts[target.index()]
            .variants
            .iter()
            .position(|v| v.name == name)
            .ok_or_else(|| format!("Wasm: codec Value lacks {name}"))?;
        let value = self.enum_value(self.key.node, target, index as u32, payload)?;
        self.copy(value, 0, input, LOC_BYTES);
        Ok(value)
    }

    pub fn codec_encode_native(&mut self) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        if args.len() != 4
            || self.mir.types[args[1].index()].constructor != T::TypeOf
            || self.mir.types[args[1].index()].arguments != [args[3]]
        {
            return Err("Wasm: codec encode signature mismatch".into());
        }
        let input = self.parameter(2);
        let properties = self.parameter(0);
        let properties = self.codec_property_context(args[0], properties)?;
        self.codec_encode_call(args[2], args[3], input, properties)
    }

    pub(crate) fn codec_encode_scalar(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        self.codec_encode_call(source, target, input, 0)
    }

    fn codec_encode_call(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
        properties: u32,
    ) -> Result<u32, String> {
        let key = Key {
            special: Special::Encode(source, target),
            callable: true,
            ..self.plan.root
        };
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: closed encoder was not planned")?;
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(properties),
            I::LocalGet(input),
            I::Call(function),
            I::LocalSet(result),
        ]);
        self.checked(result);
        Ok(result)
    }

    pub(crate) fn codec_encode_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        if source == target {
            return Ok(input);
        }
        let kind = &self.mir.types[source.index()].constructor;
        if matches!(kind, T::Nominal(_) | T::Record(_)) {
            return self.codec_encode_record(source, target, input);
        }
        if *kind == T::Never {
            self.emit(I::Unreachable);
            return Ok(self.local(ValType::I32));
        }
        if matches!(kind, T::Array | T::Dict) {
            return self.codec_encode_array(source, target, input);
        }
        if *kind == T::Tuple {
            return self.codec_encode_tuple(source, target, input);
        }
        let tag = match kind {
            T::Int => Some("Int"),
            T::Float => Some("Float"),
            T::String => Some("String"),
            T::Bytes => Some("Bytes"),
            _ => None,
        };
        if let Some(tag) = tag {
            let branch = self.plan.layouts[target.index()]
                .variants
                .iter()
                .find(|v| v.name == tag)
                .ok_or("Wasm: incomplete codec scalar contract")?;
            if branch.type_id != Some(source.index()) {
                return Err("Wasm: codec scalar payload identity mismatch".into());
            }
            return self.codec_variant(target, tag, Some(input), input);
        }
        if *kind == T::Bool {
            let output = self.local(ValType::I32);
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::If(BlockType::Empty),
            ]);
            let yes = self.codec_variant(target, "True", None, input)?;
            self.extend([I::LocalGet(yes), I::LocalSet(output), I::Else]);
            let no = self.codec_variant(target, "False", None, input)?;
            self.extend([I::LocalGet(no), I::LocalSet(output), I::End]);
            return Ok(output);
        }
        if *kind == T::Option {
            let inner = self.mir.types[source.index()].arguments[0];
            let output = self.local(ValType::I32);
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::If(BlockType::Empty),
            ]);
            let payload = self.enum_payload(source, 1, input)?;
            let present = self.codec_encode_scalar(inner, target, payload)?;
            self.extend([I::LocalGet(present), I::LocalSet(output), I::Else]);
            let absent = self.codec_variant(target, "None", None, input)?;
            self.extend([I::LocalGet(absent), I::LocalSet(output), I::End]);
            return Ok(output);
        }
        if matches!(kind, T::Result | T::FoldControl | T::PropertyTarget) {
            return self.codec_encode_enum(source, target, input);
        }
        // The codec API accepts these closed inputs, but defines a language
        // failure when invoked. Keep that failure deferred with its callback.
        let rejection = match kind {
            T::Function => Some("Function has no JSON codec"),
            T::Type | T::TypeOf => Some("cannot encode Type"),
            _ => None,
        };
        if let Some(message) = rejection {
            self.codec_error(input, message)?;
            return Ok(self.local(ValType::I32));
        }
        Err(format!(
            "Wasm: codec encode not yet implemented for sealed type {source:?}"
        ))
    }
}
