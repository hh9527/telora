use super::*;
use crate::syntax::kinds::BindingKind as B;

pub struct LoweredModule {
    pub body: HirId,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn lower_module(
    mir: &mut Mir,
    module: ModuleId,
    source: SourceId,
    cst: &CstData,
) -> LoweredModule {
    let mut lower = Lower {
        mir,
        module,
        source,
        cst,
        plans: RefCell::new(vec![]),
        needs_display: Cell::new(false),
        errors: RefCell::new(vec![]),
    };
    let syntax = lower
        .child(NodeRef::ROOT, Rule::ModuleBody)
        .unwrap_or(NodeRef::ROOT);
    let body = lower.run(syntax, Mode::Module);
    lower.finish_module(body, syntax);
    LoweredModule {
        body,
        diagnostics: lower.errors.into_inner(),
    }
}

impl Lower<'_> {
    pub(super) fn module_body(&self, node: NodeRef) -> Result<Shape, ()> {
        let mut inputs = vec![];
        for child in self.cst.children(node) {
            if Expr::cast(self.cst, child).is_some() {
                inputs.push(Input::expr(Role::Result, child));
            } else if self.rule(child).is_some() {
                match self.binding_inputs(child, true) {
                    Ok(bindings) => inputs.extend(bindings),
                    Err(()) => {}
                }
            }
        }
        Ok(Shape::Node(HirKind::Block, inputs))
    }

    fn finish_module(&mut self, body: HirId, syntax: NodeRef) {
        let mut fields = vec![];
        let mut public_names = std::collections::HashMap::new();
        for edge in &self.mir.hir[body.index()].children {
            if edge.role != Role::Binding {
                continue;
            }
            let binding = &self.mir.hir[edge.node.index()];
            let HirKind::Binding {
                kind: B::Export,
                imported: Some(local),
                ..
            } = &binding.kind
            else {
                continue;
            };
            let name_id = binding
                .children
                .iter()
                .find(|edge| edge.role == Role::Name)
                .expect("binding name")
                .node;
            let name = &self.mir.hir[name_id.index()];
            let HirKind::Name(public) = &name.kind else {
                unreachable!()
            };
            if let Some(first) = public_names.insert(public.clone(), name.location) {
                self.errors.borrow_mut().push(
                    Diagnostic::error(format!("duplicate export {public:?}"), name.location)
                        .with_secondary("first exported here", first),
                );
                continue;
            }
            let origin = match binding.origin {
                Some(HirOrigin::Source(node) | HirOrigin::Desugared(node)) => node,
                _ => syntax,
            };
            let key = self.synthetic(Role::Name, origin, HirKind::Name(public.clone()), vec![]);
            let value = self.synthetic(
                Role::Value,
                origin,
                HirKind::Variable(local.clone()),
                vec![],
            );
            fields.push(self.synthetic(Role::Field, origin, HirKind::DictField, vec![key, value]));
        }
        let has_result = self.mir.hir[body.index()]
            .children
            .iter()
            .any(|edge| edge.role == Role::Result);
        if !has_result {
            let result = self.synthetic(Role::Result, syntax, HirKind::Dict, fields);
            let node = self.run(result.node, result.mode);
            self.mir.hir[body.index()].children.push(Edge {
                role: Role::Result,
                node,
            });
        }
        if self.needs_display.get() {
            let name = self.synthetic(
                Role::Name,
                syntax,
                HirKind::Name("\0interpolation_display".into()),
                vec![],
            );
            let value = self.synthetic(
                Role::Value,
                syntax,
                HirKind::String("std/fmt".into()),
                vec![],
            );
            let binding = self.synthetic(
                Role::Binding,
                syntax,
                HirKind::Binding {
                    kind: B::Import,
                    initializer: None,
                    imported: Some("Display".into()),
                },
                vec![name, value],
            );
            let node = self.run(binding.node, binding.mode);
            self.mir.hir[body.index()].children.insert(
                0,
                Edge {
                    role: Role::Binding,
                    node,
                },
            );
        }
    }
}
