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
        let public = self.child(node, Rule::Visibility).is_ok();
        if public && !top {
            return Err(self.error(
                node,
                "pub declarations are allowed only at module top level",
            ));
        }
        match self.rule(node) {
            Some(Rule::LetPatternBinding | Rule::LetElseBinding) if top => Err(self.error(
                node,
                "destructuring let is allowed only inside a local block",
            )),
            Some(Rule::LetBinding) if public => {
                Err(self.error(node, "pub let is not supported; use pub def"))
            }
            Some(Rule::ModuleDeclaration) => self.module_declaration(node, public),
            Some(Rule::UseBinding) => self.use_binding(node, public),
            Some(Rule::DataBinding) => self.data_binding(node, public),
            Some(Rule::TraitBinding | Rule::ImplBinding) if !top => Err(self.error(
                node,
                "trait and impl declarations are allowed only at module top level",
            )),
            Some(
                Rule::DeclBinding
                | Rule::DefBinding
                | Rule::NativeBinding
                | Rule::NativeTypeBinding
                | Rule::TypeBinding
                | Rule::TraitBinding,
            ) if public => {
                let name = self.required_token(node, Token::Identifier)?;
                Ok(vec![
                    Input::with(Role::Binding, node, Mode::Binding),
                    self.export_marker(node, name, name),
                ])
            }
            _ => Ok(vec![Input::with(Role::Binding, node, Mode::Binding)]),
        }
    }

    fn module_declaration(&self, node: NodeRef, public: bool) -> Result<Vec<Input>, ()> {
        let name = self.required_token(node, Token::Identifier)?;
        let owner = self
            .mir
            .modules
            .get(self.module.index())
            .map(|module| module.name.as_str())
            .unwrap_or(self.mir.sources.get(self.source).name.as_ref());
        let child = format!("{owner}/{}", self.text(name));
        let value = self.synthetic(Role::Value, node, HirKind::String(child), vec![]);
        let binding = self.synthetic(
            Role::Binding,
            node,
            HirKind::Binding {
                kind: B::Import,
                initializer: None,
                imported: None,
            },
            vec![Input::with(Role::Name, name, Mode::Name), value],
        );
        let mut bindings = vec![binding];
        if public {
            bindings.push(self.export_marker(node, name, name));
        }
        Ok(bindings)
    }

    fn use_binding(&self, node: NodeRef, public: bool) -> Result<Vec<Input>, ()> {
        let path = self.child(node, Rule::UsePath)?;
        let base: Vec<String> = self
            .cst
            .children(path)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)))
            .map(|name| self.text(name).into_owned())
            .collect();
        let make = |origin, local, imported, segments: Vec<String>| {
            let value = self.synthetic(Role::Value, origin, HirKind::StaticPath(segments), vec![]);
            self.synthetic(
                Role::Binding,
                origin,
                HirKind::Binding {
                    kind: B::Import,
                    initializer: None,
                    imported: Some(imported),
                },
                vec![Input::with(Role::Name, local, Mode::Name), value],
            )
        };
        if let Ok(selector) = self.child(node, Rule::UseSelector) {
            let items = self.child(selector, Rule::UseItems)?;
            let mut bindings = vec![];
            for item in self
                .cst
                .children(items)
                .filter(|child| self.rule(*child) == Some(Rule::UseItem))
            {
                let (imported, local) = self.selector_names(item)?;
                let imported_name = self.text(imported).into_owned();
                let same_local = base.as_slice() == ["self"]
                    && self.text(imported).as_ref() == self.text(local).as_ref();
                if !same_local {
                    let module_path = matches!(
                        base.first().map(String::as_str),
                        Some("crate" | "self" | "super")
                    ) || base.first().is_some_and(|first| {
                        self.mir.modules.iter().any(|module| module.name == *first)
                    });
                    let value = if base.as_slice() == ["self"] {
                        self.synthetic(
                            Role::Value,
                            item,
                            HirKind::Variable(imported_name.clone()),
                            vec![],
                        )
                    } else if module_path && base.len() == 1 {
                        let mut path = base.clone();
                        path.push(imported_name.clone());
                        self.synthetic(Role::Value, item, HirKind::StaticPath(path), vec![])
                    } else {
                        let receiver = self.synthetic(
                            Role::Receiver,
                            item,
                            HirKind::StaticPath(base.clone()),
                            vec![],
                        );
                        self.synthetic(
                            Role::Value,
                            item,
                            HirKind::Field,
                            vec![receiver, Input::with(Role::Name, imported, Mode::Name)],
                        )
                    };
                    bindings.push(self.synthetic(
                        Role::Binding,
                        item,
                        HirKind::Binding {
                            kind: if module_path { B::Import } else { B::Def },
                            initializer: None,
                            imported: Some(imported_name),
                        },
                        vec![Input::with(Role::Name, local, Mode::Name), value],
                    ));
                }
                if public {
                    bindings.push(self.export_marker(item, local, local));
                }
            }
            return Ok(bindings);
        }
        let Some(local) = self
            .cst
            .children(path)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)))
            .last()
        else {
            return Err(self.error(path, "use path requires a binding name"));
        };
        let imported = self.text(local).into_owned();
        let alias = self
            .cst
            .children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)))
            .last()
            .unwrap_or(local);
        let same_local = base.first().is_some_and(|segment| segment == "self")
            && base.len() == 2
            && self.text(local).as_ref() == self.text(alias).as_ref();
        let mut bindings = if same_local {
            vec![]
        } else {
            vec![make(node, alias, imported, base)]
        };
        if public {
            bindings.push(self.export_marker(node, alias, alias));
        }
        Ok(bindings)
    }

    fn data_binding(&self, node: NodeRef, public: bool) -> Result<Vec<Input>, ()> {
        if self.child(node, Rule::TypeScheme).is_ok() {
            return Err(self.error(node, "typed data declarations are unsupported yet"));
        }
        let name = self.required_token(node, Token::Identifier)?;
        let import = self.child(node, Rule::DataImport)?;
        let source = self.child(import, Rule::StringLiteral)?;
        let source = self.plain_string(source)?;
        let owner = self
            .mir
            .modules
            .get(self.module.index())
            .map(|module| module.name.as_str())
            .unwrap_or(self.mir.sources.get(self.source).name.as_ref());
        let request = data_request(owner, &source)
            .ok_or_else(|| self.error(node, "data source escapes its crate boundary"))?;
        let request = self.synthetic(Role::Value, node, HirKind::String(request), vec![]);
        let binding = self.synthetic(
            Role::Binding,
            node,
            HirKind::Binding {
                kind: B::Import,
                initializer: None,
                imported: Some("data".into()),
            },
            vec![Input::with(Role::Name, name, Mode::Name), request],
        );
        let mut bindings = vec![binding];
        if public {
            bindings.push(self.export_marker(node, name, name));
        }
        Ok(bindings)
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

    fn selector_names(&self, node: NodeRef) -> Result<(NodeRef, NodeRef), ()> {
        let mut names = self
            .cst
            .children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)));
        let first = names.next().ok_or(())?;
        Ok((first, names.next().unwrap_or(first)))
    }
}

fn data_request(owner: &str, source: &str) -> Option<String> {
    if source.starts_with('@') {
        return Some(source.to_owned());
    }
    let mut path = owner.split('/').collect::<Vec<_>>();
    // Data paths are relative to the declaring source file. A crate root is
    // `src/lib.telora`, while every other module name includes that file's
    // extension-free path.
    if path.len() > 1 {
        path.pop();
    }
    for part in source.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if path.len() <= 1 {
                    return None;
                }
                path.pop();
            }
            part => path.push(part),
        }
    }
    Some(path.join("/"))
}

#[cfg(test)]
mod tests {
    use super::data_request;

    #[test]
    fn data_paths_are_relative_to_the_declaring_source_file() {
        assert_eq!(
            data_request("app", "./config.json").as_deref(),
            Some("app/config.json")
        );
        assert_eq!(
            data_request("app/query", "./input.json").as_deref(),
            Some("app/input.json")
        );
        assert_eq!(
            data_request("app/query/parser", "../input.json").as_deref(),
            Some("app/input.json")
        );
        assert_eq!(data_request("app/query", "../../outside.json"), None);
    }
}
