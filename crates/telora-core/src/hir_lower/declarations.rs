use super::*;
use crate::mir::TypeOperation;
use crate::syntax::kinds::{BindingKind as B, DeclaredInitializerKind as D};

impl Lower<'_> {
    pub(super) fn declaration(&self, node: NodeRef) -> Result<Shape, ()> {
        let rule = self
            .rule(node)
            .ok_or_else(|| self.error(node, "invalid binding"))?;
        let name = if rule == Rule::ImplBinding {
            self.synthetic(
                Role::Name,
                node,
                HirKind::Name(format!("\0trait_impl_{}", self.location(node).start)),
                vec![],
            )
        } else {
            Input::with(
                Role::Name,
                self.required_token(node, Token::Identifier)?,
                Mode::Name,
            )
        };
        let mut inputs = vec![name];
        let scheme = self.child(node, Rule::TypeScheme).ok();
        let parameters = self
            .child(scheme.unwrap_or(node), Rule::TypeParameters)
            .ok();
        if let Some(parameters) = parameters {
            inputs.extend(
                self.cst
                    .children(parameters)
                    .filter(|child| self.rule(*child) == Some(Rule::TypeParameter))
                    .map(|node| Input::with(Role::TypeParameter, node, Mode::TypeParameter)),
            );
        }
        inputs.extend(self.decorators(node));
        let mut initializer = None;
        let kind = match rule {
            Rule::DeclBinding | Rule::NativeBinding | Rule::DefBinding => {
                if let Some(scheme) = scheme {
                    let contract = self.contract_child(scheme)?;
                    inputs.push(Input::with(Role::Annotation, contract, Mode::Contract));
                    if rule != Rule::DefBinding {
                        inputs.push(Input::with(Role::Value, contract, Mode::Contract));
                    }
                } else if rule != Rule::DefBinding {
                    return Err(self.error(node, "declaration has no type scheme"));
                }
                if rule == Rule::DefBinding {
                    let equal = self.required_token(node, Token::Equal)?;
                    let value = self
                        .expressions(node)
                        .into_iter()
                        .find(|child| child.0 > equal.0);
                    inputs.push(self.optional_expression(Role::Value, node, value));
                }
                match rule {
                    Rule::DefBinding => B::Def,
                    Rule::DeclBinding => B::Decl,
                    _ => B::Native,
                }
            }
            Rule::NativeTypeBinding => {
                let slot = self.required_token(node, Token::Int)?;
                let value = self
                    .text(slot)
                    .parse()
                    .map_err(|_| self.error(slot, "native type slot is outside the i64 range"))?;
                inputs.push(self.synthetic(
                    Role::Value,
                    slot,
                    HirKind::NativeTypeSlot(value),
                    vec![],
                ));
                B::NativeType
            }
            Rule::TypeBinding => {
                if let Some(value) = self.cst.children(node).find(|child| {
                    matches!(
                        self.rule(*child),
                        Some(Rule::StructInitializer | Rule::EnumInitializer)
                    )
                }) {
                    initializer = Some(match self.rule(value).unwrap() {
                        Rule::EnumInitializer => D::Enum,
                        _ if self.token(value, Token::LParen).is_some() => D::Newtype,
                        _ => D::Struct,
                    });
                    inputs.push(Input::with(Role::Value, value, Mode::TypeInitializer));
                } else {
                    let value = self.first_expression(node)?;
                    inputs.push(Input::with(Role::Value, value, Mode::Type));
                }
                B::Type
            }
            Rule::TraitBinding => {
                initializer = Some(D::Struct);
                let name = self.synthetic(Role::Name, node, HirKind::Name("Self".into()), vec![]);
                inputs.push(self.synthetic(
                    Role::TypeParameter,
                    node,
                    HirKind::TypeParameter,
                    vec![name],
                ));
                inputs.push(Input::with(Role::Value, node, Mode::TypeInitializer));
                B::Trait
            }
            Rule::ImplBinding => {
                let contracts = self.contract_parts(node);
                let [trait_type, target] = contracts.as_slice() else {
                    return Err(self.error(node, "impl requires a trait and target type"));
                };
                inputs.push(self.synthetic(
                    Role::Annotation,
                    node,
                    HirKind::Call,
                    vec![
                        Input::with(Role::Callee, *trait_type, Mode::Contract),
                        Input::with(Role::Argument, *target, Mode::Contract),
                    ],
                ));
                let mut fields = vec![];
                let mut names = std::collections::HashSet::new();
                for member in self
                    .cst
                    .children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::ImplMember))
                {
                    let name = self.required_token(member, Token::Identifier)?;
                    if !names.insert(self.text(name).into_owned()) {
                        return Err(self
                            .error(name, format!("duplicate impl member {:?}", self.text(name))));
                    }
                    let value = self.first_expression(member)?;
                    fields.push(self.synthetic(
                        Role::Field,
                        member,
                        HirKind::DictField,
                        vec![
                            Input::with(Role::Name, name, Mode::Name),
                            Input::expr(Role::Value, value),
                        ],
                    ));
                }
                inputs.push(self.synthetic(Role::Value, node, HirKind::Dict, fields));
                B::Impl
            }
            _ => return Err(self.error(node, "unexpected binding rule")),
        };
        Ok(Shape::Node(
            HirKind::Binding {
                kind,
                initializer,
                imported: None,
            },
            inputs,
        ))
    }

    pub(super) fn type_parameter(&self, node: NodeRef) -> Result<Shape, ()> {
        let name = self.required_token(node, Token::Identifier)?;
        let mut inputs = vec![Input::with(Role::Name, name, Mode::Name)];
        for bound in self
            .cst
            .children(node)
            .filter(|child| self.rule(*child) == Some(Rule::TraitBound))
        {
            inputs.push(Input::with(
                Role::Bound,
                self.contract_child(bound)?,
                Mode::Contract,
            ));
        }
        Ok(Shape::Node(HirKind::TypeParameter, inputs))
    }

    pub(super) fn type_initializer(&self, node: NodeRef) -> Result<Shape, ()> {
        let operation = match self.rule(node) {
            Some(Rule::StructInitializer) if self.token(node, Token::LParen).is_some() => {
                let payload = self.first_expression(node)?;
                let member = self.synthetic(
                    Role::Field,
                    payload,
                    HirKind::TypeMember {
                        name: "payload".into(),
                        nullary: false,
                    },
                    vec![Input::with(Role::Annotation, payload, Mode::Type)],
                );
                return Ok(Shape::Desugared(
                    HirKind::TypeOperation(TypeOperation::Newtype),
                    vec![member],
                ));
            }
            Some(Rule::StructInitializer | Rule::TraitBinding) => TypeOperation::Struct,
            Some(Rule::EnumInitializer) => TypeOperation::Enum,
            _ => return Err(self.error(node, "invalid type initializer")),
        };
        let mut names = std::collections::HashSet::new();
        let mut inputs = vec![];
        for member in self.cst.children(node).filter(|child| {
            matches!(
                self.rule(*child),
                Some(
                    Rule::StructInitializerField | Rule::EnumInitializerVariant | Rule::TraitMember
                )
            )
        }) {
            let name = self.required_token(member, Token::Identifier)?;
            if !names.insert(self.text(name).into_owned()) {
                let label = match self.rule(member).unwrap() {
                    Rule::TraitMember => "trait member",
                    Rule::EnumInitializerVariant => "Enum variant",
                    _ => "Struct field",
                };
                return Err(self.error(name, format!("duplicate {label} {:?}", self.text(name))));
            }
            inputs.push(Input::with(Role::Field, member, Mode::TypeMember));
        }
        Ok(Shape::Desugared(HirKind::TypeOperation(operation), inputs))
    }

    pub(super) fn type_member(&self, node: NodeRef) -> Result<Shape, ()> {
        let name = self.required_token(node, Token::Identifier)?;
        let nullary = self.rule(node) == Some(Rule::EnumInitializerVariant)
            && self.token(node, Token::LParen).is_none();
        let mut inputs = self.decorators(node);
        if !nullary {
            let (value, mode) = if self.rule(node) == Some(Rule::TraitMember) {
                (self.contract_child(node)?, Mode::Contract)
            } else {
                (self.first_expression(node)?, Mode::Type)
            };
            inputs.push(Input::with(Role::Annotation, value, mode));
        }
        Ok(Shape::Node(
            HirKind::TypeMember {
                name: self.text(name).into_owned(),
                nullary,
            },
            inputs,
        ))
    }
}
