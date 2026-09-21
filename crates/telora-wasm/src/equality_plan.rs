//! A finite set of type-specialized comparers, including recursive type graphs.
use crate::plan::{Key, Plan, Special, child};
use telora_core::{
    candidate_layout::State,
    mir::{HirKind, Role, SealedExecutable, TypeConstructor as T},
    syntax::kinds::BinaryOperator as B,
};

impl Plan {
    pub(crate) fn plan_comparisons(
        &mut self,
        executable: &SealedExecutable<'_>,
    ) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        let mut pending = Vec::new();
        for root in executable.closure().nodes() {
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            if matches!(
                mir.hir[root.node.index()].kind,
                HirKind::Binary(B::Equal | B::NotEqual)
            ) {
                let left = key.effective_ty(mir, child(mir, root.node, Role::Left)?)?;
                let right = key.effective_ty(mir, child(mir, root.node, Role::Right)?)?;
                if left == right {
                    pending.push(left);
                }
            } else if crate::natives::identity(mir, root.node) == Some((1, "equal")) {
                let signature = &mir.types[key.ty(mir, root.node)?.index()];
                if signature.arguments.len() != 3
                    || signature.arguments[0] != signature.arguments[1]
                    || mir.types[signature.arguments[2].index()].constructor != T::Bool
                {
                    return Err(
                        "Wasm: equality native lacks a closed Fn(T, T) -> Bool signature".into(),
                    );
                }
                pending.push(signature.arguments[0]);
            }
        }
        while let Some(ty) = pending.pop() {
            if self.comparisons.contains_key(&ty) {
                continue;
            }
            let key = Key {
                callable: true,
                special: Special::Equal(ty),
                ..self.root
            };
            self.comparisons.insert(ty, key);
            self.functions.insert(key, 0);
            if matches!(self.layouts[ty.index()].layout, State::Uninhabited { .. }) {
                continue;
            }
            if matches!(mir.types[ty.index()].constructor, T::Array | T::Dict) {
                pending.push(mir.types[ty.index()].arguments[0]);
            } else if !self.layouts[ty.index()].variants.is_empty() {
                pending.extend(
                    self.layouts[ty.index()]
                        .variants
                        .iter()
                        .filter_map(|v| v.type_id)
                        .map(|id| self.layouts[id].id()),
                );
            } else if matches!(&self.layouts[ty.index()].layout, State::Known { shape }
                if matches!(shape.table, Some("RecordTable" | "NewtypeTable")))
            {
                let object = self.layouts[ty.index()]
                    .object
                    .as_ref()
                    .ok_or("Wasm: equality object layout missing")?;
                for member in &object.members {
                    pending.push(
                        self.layouts[member.type_id.ok_or("Wasm: equality field type missing")?]
                            .id(),
                    );
                }
            }
        }
        Ok(())
    }
}
