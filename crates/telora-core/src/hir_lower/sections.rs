use super::*;

impl Lower<'_> {
    pub(super) fn section(&self, node: NodeRef) -> Result<Shape, ()> {
        let [callee] = self.operands(node)?;
        let args_node = self.child(node, Rule::SectionArguments)?;
        let mut args = vec![];
        let mut bare = vec![];
        let mut indexed = vec![];
        for arg in self
            .cst
            .children(args_node)
            .filter(|child| self.rule(*child) == Some(Rule::Argument))
        {
            if let Some(placeholder) = self.token(arg, Token::Placeholder) {
                args.push((placeholder, Some(bare.len())));
                bare.push(placeholder);
            } else if let Some(placeholder) = self.token(arg, Token::IndexedPlaceholder) {
                let index: usize = self.text(placeholder)[1..].parse().map_err(|_| {
                    self.error(placeholder, "placeholder index exceeds the supported range")
                })?;
                args.push((placeholder, Some(index)));
                indexed.push((placeholder, index));
            } else {
                args.push((self.first_expression(arg)?, None));
            }
        }
        if !bare.is_empty() && !indexed.is_empty() {
            return Err(self.error(
                indexed[0].0,
                "cannot mix '_' and indexed placeholders in one call",
            ));
        }
        if bare.is_empty() && indexed.is_empty() {
            return Err(self.error(node, "call section requires at least one placeholder"));
        }
        let parameters = if indexed.is_empty() {
            bare
        } else {
            let max = indexed.iter().map(|(_, index)| *index).max().unwrap();
            if max >= u16::MAX as usize {
                let location = indexed.iter().find(|(_, index)| *index == max).unwrap().0;
                return Err(self.error(
                    location,
                    format!(
                        "placeholder index exceeds the limit of {} parameters",
                        u16::MAX
                    ),
                ));
            }
            let mut slots = vec![None; max + 1];
            for (node, index) in &indexed {
                slots[*index].get_or_insert(*node);
            }
            if let Some(missing) = slots.iter().position(Option::is_none) {
                return Err(self.error(
                    indexed[0].0,
                    format!("indexed placeholders are missing _{missing}"),
                ));
            }
            slots.into_iter().map(Option::unwrap).collect()
        };
        let mut inputs = vec![];
        for (index, parameter) in parameters.into_iter().enumerate() {
            let name = self.synthetic(
                Role::Name,
                parameter,
                HirKind::Name(format!("\0telora_placeholder_{index}")),
                vec![],
            );
            inputs.push(self.synthetic(Role::Parameter, parameter, HirKind::Parameter, vec![name]));
        }
        inputs.push(self.synthetic(Role::ReturnType, node, HirKind::ReturnType, vec![]));
        let mut call = vec![Input::expr(Role::Callee, callee)];
        for (arg, index) in args {
            call.push(if let Some(index) = index {
                self.synthetic(
                    Role::Argument,
                    arg,
                    HirKind::Variable(format!("\0telora_placeholder_{index}")),
                    vec![],
                )
            } else {
                Input::expr(Role::Argument, arg)
            });
        }
        let call = self.synthetic(Role::Result, node, HirKind::Call, call);
        inputs.push(self.synthetic(Role::Body, node, HirKind::Block, vec![call]));
        Ok(Shape::Desugared(HirKind::Closure, inputs))
    }

    pub(super) fn type_apply(&self, node: NodeRef) -> Result<Shape, ()> {
        let [callee] = self.operands(node)?;
        let args = self.child(node, Rule::TypeArguments)?;
        let mut inputs = vec![Input::expr(Role::Callee, callee)];
        for arg in self
            .cst
            .children(args)
            .filter(|child| self.rule(*child) == Some(Rule::TypeArgument))
        {
            if self.token(arg, Token::Placeholder).is_some() {
                inputs.push(self.synthetic(
                    Role::Argument,
                    arg,
                    HirKind::InferredTypeArgument,
                    vec![],
                ));
            } else {
                let value = self.first_expression(arg)?;
                inputs.push(Input::with(Role::Argument, value, Mode::Type));
            }
        }
        Ok(Shape::Node(HirKind::TypeApply, inputs))
    }
}
