//! Test-only destructive stage probe. A private builder copies borrowed input
//! once; every callback receives a stable scalar snapshot, never a mutable slot.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(super) fn alias_array_rewrite(
        &mut self,
        name: &str,
        states: u32,
        state: TypeId,
        result: TypeId,
        output: TypeId,
    ) -> Result<u32, String> {
        let noescape = name.ends_with("_noescape")
            || (name.ends_with("_checked")
                && crate::collection_summaries::scalar_callbacks_safe(self.mir, self.plan, name));
        let name = name
            .strip_suffix("_noescape")
            .or_else(|| name.strip_suffix("_checked"))
            .unwrap_or(name);
        let name = name.strip_suffix("_generic").unwrap_or(name);
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        if args.len() != 3 || self.mir.types[output.index()].constructor != T::Bool {
            return Err("array probe signature".into());
        }
        let item = self.mir.types[result.index()].arguments[0];
        let callback_ty = args[1];
        let callback_args = &self.mir.types[callback_ty.index()].arguments;
        if (self.mir.types[item.index()].constructor != T::Int && name != "alias_filter_array")
            || self.mir.types[callback_ty.index()].constructor != T::Function
            || callback_args.len() != 2
            || callback_args[0] != item
        {
            return Err("array probe supports Int maps and scalar-result predicates".into());
        }
        let callback_result = callback_args[1];
        let valid = match name {
            "alias_map_array" => callback_result == item,
            "alias_filter_array" => self.mir.types[callback_result.index()].constructor == T::Bool,
            "alias_filtermap_array" => {
                self.mir.types[callback_result.index()].constructor == T::Option
                    && self.mir.types[callback_result.index()].arguments == [item]
            }
            _ => false,
        };
        if !valid {
            return Err("array probe callback result".into());
        }
        let (base, _) = self.array_parts(states, self.width(state)?);
        let (entries, _) = self.sequence_field(base, state, "entries")?;
        let (sealed, _) = self.sequence_field(base, state, "sealed")?;
        self.bits(sealed);
        self.extend([I::I64Eqz, I::I32Eqz]);
        self.fail_if(self.key.node, ERROR_UNINITIALIZED_CALL);
        let width = self.width(item)?;
        let (data, count) = self.array_parts(entries, width);
        let callback = self.parameter(1);
        let index = self.local(ValType::I32);
        let written = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let source = self.array_item(data, index, width);
        // Even an Int can escape through a callback's closure or memo. Never
        // let such a reference point at a slot subsequently overwritten here.
        let snapshot = if noescape {
            source
        } else {
            let snapshot = self.alloc(width);
            self.copy(snapshot, 0, source, width);
            self.root_value(snapshot, item);
            snapshot
        };
        let mapped = self.invoke(callback, &[snapshot], callback_result)?;
        let value = match name {
            "alias_map_array" => mapped,
            "alias_filter_array" => {
                self.bits(mapped);
                self.extend([I::I64Eqz, I::I32Eqz, I::If(BlockType::Empty)]);
                snapshot
            }
            _ => {
                self.extend([
                    I::LocalGet(mapped),
                    I::I32Load(memory(DATA, 2)),
                    I::If(BlockType::Empty),
                ]);
                self.enum_payload(callback_result, 1, mapped)?
            }
        };
        let destination = self.array_item(data, written, width);
        self.copy(destination, 0, value, width);
        self.extend([
            I::LocalGet(written),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(written),
        ]);
        if name != "alias_map_array" {
            self.emit(I::End);
        }
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.array_payload(entries, data, written, count, item)?;
        self.scalar_as(self.key.node, output, 1)
    }
}
