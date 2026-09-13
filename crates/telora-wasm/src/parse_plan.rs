//! Finite type-specialized parsers, including recursive record graphs.
use crate::plan::{Key, Plan, Special};
use telora_core::mir::{SealedExecutable, TypeConstructor as T};
impl Plan {
    pub(crate) fn plan_parsers(&mut self, executable: &SealedExecutable<'_>) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        let mut pending = Vec::new();
        for root in executable.closure().nodes() {
            if crate::natives::identity(mir, root.node) != Some((7, "parse_with")) {
                continue;
            }
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let signature = &mir.types[key.ty(mir, root.node)?.index()];
            let metadata = *signature
                .arguments
                .get(1)
                .ok_or("Wasm: parse signature missing metadata")?;
            pending.push(
                *mir.types[metadata.index()]
                    .arguments
                    .first()
                    .ok_or("Wasm: parse target missing")?,
            );
        }
        while let Some(ty) = pending.pop() {
            if self.parsers.contains_key(&ty) {
                continue;
            }
            let key = Key {
                special: Special::Parse(ty),
                callable: true,
                ..self.root
            };
            self.parsers.insert(ty, key);
            self.functions.insert(key, 0);
            if mir.types[ty.index()].constructor == T::Option {
                pending.extend(&mir.types[ty.index()].arguments);
            } else if let Some(object) = &self.layouts[ty.index()].object {
                pending.extend(
                    object
                        .members
                        .iter()
                        .filter_map(|m| m.type_id)
                        .map(|id| self.layouts[id].id()),
                );
            }
        }
        Ok(())
    }
}
