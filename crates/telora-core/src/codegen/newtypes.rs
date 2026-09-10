use super::*;

impl Emitter<'_> {
    pub(super) fn newtype_owner(&self, node: HirId) -> Result<Option<TypeId>, Diagnostic> {
        if !matches!(self.mir.member_selections[node.index()], Some(MemberSelection::NewtypeConstructor)) {
            return Ok(None);
        }
        let signature = &self.mir.types[self.ty(node)?.index()];
        if signature.constructor != TypeConstructor::Function || signature.arguments.len() != 2 {
            return Err(self.error(node, "newtype constructor has no closed unary signature"));
        }
        Ok(Some(signature.arguments[1]))
    }

    pub(super) fn newtype_constructor(
        &mut self,
        node: HirId,
        owner: TypeId,
    ) -> Result<R, Diagnostic> {
        let mut pending = vec![owner];
        while let Some(id) = pending.pop() {
            let ty = &self.mir.types[id.index()];
            if matches!(ty.constructor, TypeConstructor::Parameter(_)) {
                return Err(self.error(
                    node,
                    "generic newtype construction requires a compiled type witness",
                ));
            }
            pending.extend(ty.arguments.iter().copied());
        }
        let mut constructor = Self::new(self.mir, self.graph, format!("newtype:{}", owner.index()));
        constructor.function.parameter_count = 1;
        let payload = constructor.register();
        constructor.construction_check(node, owner, PropertySite::Type, payload);
        let dst = constructor.register();
        constructor.emit(
            node,
            O::MakeNewtype {
                dst,
                ty: owner,
                payload,
            },
        );
        constructor.emit(node, O::Return { src: dst });
        let dst = self.register();
        self.emit(
            node,
            O::MakeClosure {
                dst,
                function: Box::new(constructor.function),
                captures: vec![],
            },
        );
        Ok(dst)
    }
}
