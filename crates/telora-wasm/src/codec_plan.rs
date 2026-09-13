//! One encoding function per closed (source, Value) identity pair.
use crate::plan::{Key, Plan, Special};
use telora_core::mir::{SealedExecutable, TypeConstructor as T};

impl Plan {
    pub(crate) fn plan_encoders(
        &mut self,
        executable: &SealedExecutable<'_>,
    ) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        let mut pending = Vec::new();
        for root in executable.closure().nodes() {
            if crate::natives::identity(mir, root.node) != Some((13, "encode_with")) {
                continue;
            }
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let args = &mir.types[key.ty(mir, root.node)?.index()].arguments;
            if args.len() != 4 {
                return Err("Wasm: encoder signature mismatch".into());
            }
            pending.push((args[2], args[3]));
        }
        while let Some((source, target)) = pending.pop() {
            let key = Key {
                special: Special::Encode(source, target),
                callable: true,
                ..self.root
            };
            if self.functions.contains_key(&key) {
                continue;
            }
            self.functions.insert(key, 0);
            if source == target {
                continue;
            }
            if matches!(
                mir.types[source.index()].constructor,
                T::Array | T::Dict | T::Tuple | T::Option
            ) {
                pending.extend(
                    mir.types[source.index()]
                        .arguments
                        .iter()
                        .map(|&child| (child, target)),
                );
            }
            if let Some(object) = &self.layouts[source.index()].object {
                pending.extend(
                    object
                        .members
                        .iter()
                        .filter_map(|member| member.type_id)
                        .map(|id| (self.layouts[id].id(), target)),
                );
            }
            pending.extend(
                self.layouts[source.index()]
                    .variants
                    .iter()
                    .filter_map(|member| member.type_id)
                    .map(|id| (self.layouts[id].id(), target)),
            );
        }
        Ok(())
    }
}
