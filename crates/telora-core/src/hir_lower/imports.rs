use super::*;
use crate::syntax::kinds::BindingKind as B;

impl Lower<'_> {
    pub(super) fn binding_inputs(&self, mut node: NodeRef, top: bool) -> Result<Vec<Input>, ()> {
        if self.rule(node) == Some(Rule::Binding) {
            node = self
                .cst
                .children(node)
                .find(|child| self.rule(*child).is_some())
                .ok_or(())?;
        }
        match self.rule(node) {
            Some(Rule::LetPatternBinding | Rule::LetElseBinding) if top => Err(self.error(
                node,
                "destructuring let is allowed only inside a local block",
            )),
            Some(Rule::ImportBinding) => self.imports(node),
            Some(Rule::ExportStatement) if top => self.exports(node),
            Some(Rule::ExportStatement) => Err(self.error(
                node,
                "export declarations are allowed only at module top level",
            )),
            Some(Rule::TraitBinding | Rule::ImplBinding) if !top => Err(self.error(
                node,
                "trait and impl declarations are allowed only at module top level",
            )),
            _ => Ok(vec![Input::with(Role::Binding, node, Mode::Binding)]),
        }
    }

    fn imports(&self, node: NodeRef) -> Result<Vec<Input>, ()> {
        if let Ok(selector) = self.child(node, Rule::MemberSelector) {
            return self.member_imports(selector, false);
        }
        let path = self.child(node, Rule::StringLiteral)?;
        let request = self.plain_string(path)?;
        let selector = self.child(node, Rule::ImportSelector)?;
        let mut bindings = vec![];
        let make = |origin, name, imported, kind| {
            let value = self.synthetic(Role::Value, path, HirKind::String(request.clone()), vec![]);
            self.synthetic(
                Role::Binding,
                origin,
                HirKind::Binding {
                    kind,
                    initializer: None,
                    imported,
                },
                vec![name, value],
            )
        };
        if self.token(selector, Token::As).is_some() {
            let name = self.required_token(selector, Token::Identifier)?;
            bindings.push(make(
                node,
                Input::with(Role::Name, name, Mode::Name),
                None,
                B::Import,
            ));
        }
        if self.token(selector, Token::Star).is_some() {
            let name = self.synthetic(
                Role::Name,
                node,
                HirKind::Name(format!("\0open:{}", self.location(node).start)),
                vec![],
            );
            bindings.push(make(node, name, None, B::OpenImport));
        }
        if let Ok(items) = self.child(selector, Rule::ImportItems) {
            for item in self
                .cst
                .children(items)
                .filter(|child| self.rule(*child) == Some(Rule::ImportItem))
            {
                let (imported, local) = self.selector_names(item)?;
                bindings.push(make(
                    item,
                    Input::with(Role::Name, local, Mode::Name),
                    Some(self.text(imported).into_owned()),
                    B::Import,
                ));
            }
        }
        Ok(bindings)
    }

    fn exports(&self, node: NodeRef) -> Result<Vec<Input>, ()> {
        if let Ok(selector) = self.child(node, Rule::MemberSelector) {
            return self.member_imports(selector, true);
        }
        if let Some(binding) = self.cst.children(node).find(|child| {
            matches!(
                self.rule(*child),
                Some(Rule::LetBinding | Rule::DefBinding | Rule::TypeBinding | Rule::TraitBinding)
            )
        }) {
            if self.rule(binding) == Some(Rule::LetBinding) {
                return Err(self.error(binding, "export let is not supported; use export def"));
            }
            let name = self.required_token(binding, Token::Identifier)?;
            return Ok(vec![
                Input::with(Role::Binding, binding, Mode::Binding),
                self.export_marker(node, name, name),
            ]);
        }
        let items = self.child(node, Rule::ExportItems)?;
        self.cst
            .children(items)
            .filter(|child| self.rule(*child) == Some(Rule::ExportItem))
            .map(|item| {
                let (local, public) = self.selector_names(item)?;
                Ok(self.export_marker(item, local, public))
            })
            .collect()
    }

    fn export_marker(&self, origin: NodeRef, local: NodeRef, public: NodeRef) -> Input {
        let value = self.synthetic(
            Role::Value,
            origin,
            HirKind::Variable(self.text(local).into_owned()),
            vec![],
        );
        self.synthetic(
            Role::Binding,
            origin,
            HirKind::Binding {
                kind: B::Export,
                initializer: None,
                imported: Some(self.text(local).into_owned()),
            },
            vec![Input::with(Role::Name, public, Mode::Name), value],
        )
    }

    fn member_imports(&self, node: NodeRef, exported: bool) -> Result<Vec<Input>, ()> {
        let last = self
            .cst
            .children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)))
            .last()
            .ok_or(())?;
        let items = self.child(node, Rule::ImportItems)?;
        let mut bindings = vec![];
        for item in self
            .cst
            .children(items)
            .filter(|child| self.rule(*child) == Some(Rule::ImportItem))
        {
            let (member, local) = self.selector_names(item)?;
            let value = self.synthetic(
                Role::Value,
                item,
                HirKind::Field,
                vec![
                    Input::with(Role::Receiver, node, Mode::Path(last)),
                    Input::with(Role::Name, member, Mode::Name),
                ],
            );
            bindings.push(self.synthetic(
                Role::Binding,
                item,
                HirKind::Binding {
                    kind: B::Def,
                    initializer: None,
                    imported: Some(self.text(member).into_owned()),
                },
                vec![Input::with(Role::Name, local, Mode::Name), value],
            ));
            if exported {
                bindings.push(self.export_marker(item, local, local));
            }
        }
        Ok(bindings)
    }

    fn selector_names(&self, node: NodeRef) -> Result<(NodeRef, NodeRef), ()> {
        let mut names = self
            .cst
            .children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)));
        let first = names.next().ok_or(())?;
        Ok((first, names.next().unwrap_or(first)))
    }
}
