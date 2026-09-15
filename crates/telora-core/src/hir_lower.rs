//! CST-backed semantic lowering, developed independently of the owned AST.
//!
//! This entry point is not yet the module loader's production path. Unsupported
//! syntax is reported explicitly; it never delegates to the old lowerer.
//! Each task contains syntax references and shallow semantic state, not a tree.

use crate::mir::{Edge, HirId, HirKind, HirOrigin, Mir, ModuleId, Role};
use crate::source::{Diagnostic, Location, SourceId, TextRange};
use crate::syntax::telora::{
    ast::{AstNode, Expr, SyntaxNode},
    lexer::Token,
    parser::{CstData, Node, NodeRef, Rule},
};

mod contracts;
mod expressions;
mod scopes;
mod types;

/// Append an expression's semantic graph and its unsolved slots to the session.
/// No source references are stored as Rust borrows inside the resulting graph.
pub fn expression(
    mir: &mut Mir,
    module: ModuleId,
    source: SourceId,
    cst: &CstData,
    root: NodeRef,
) -> Result<HirId, Diagnostic> {
    Lower {
        mir,
        module,
        source,
        cst,
    }
    .run(root)
}

#[derive(Clone, Copy)]
enum Mode {
    Expression,
    Name,
    Type,
    TypeTerm,
    TypeArgument,
    Contract,
    Path(NodeRef),
    Parameter,
    ReturnSlot,
    Body,
    Binding,
    Discard,
    DiscardName,
    Unit,
}

#[derive(Clone, Copy)]
struct Input {
    role: Role,
    node: NodeRef,
    mode: Mode,
}

impl Input {
    fn expr(role: Role, node: NodeRef) -> Self {
        Self {
            role,
            node,
            mode: Mode::Expression,
        }
    }

    fn with(role: Role, node: NodeRef, mode: Mode) -> Self {
        Self { role, node, mode }
    }
}

enum Shape {
    Alias(NodeRef, Mode),
    Node(HirKind, Vec<Input>),
    Desugared(HirKind, Vec<Input>),
}

enum Task {
    Visit(NodeRef, Mode),
    Finish {
        syntax: NodeRef,
        kind: HirKind,
        origin: HirOrigin,
        roles: Vec<Role>,
        base: usize,
    },
}

struct Lower<'a> {
    mir: &'a mut Mir,
    module: ModuleId,
    source: SourceId,
    cst: &'a CstData,
}

impl Lower<'_> {
    fn run(&mut self, root: NodeRef) -> Result<HirId, Diagnostic> {
        let mut tasks = vec![Task::Visit(root, Mode::Expression)];
        let mut results = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(syntax, mode) => {
                    let shape = self.shape(syntax, mode)?;
                    let (kind, inputs, origin) = match shape {
                        Shape::Alias(node, mode) => {
                            tasks.push(Task::Visit(node, mode));
                            continue;
                        }
                        Shape::Node(kind, inputs) => (kind, inputs, HirOrigin::Source(syntax)),
                        Shape::Desugared(kind, inputs) => {
                            (kind, inputs, HirOrigin::Desugared(syntax))
                        }
                    };
                    tasks.push(Task::Finish {
                        syntax,
                        kind,
                        origin,
                        roles: inputs.iter().map(|input| input.role).collect(),
                        base: results.len(),
                    });
                    // Reverse the scheduling, not the source traversal order.
                    tasks.extend(
                        inputs
                            .into_iter()
                            .rev()
                            .map(|input| Task::Visit(input.node, input.mode)),
                    );
                }
                Task::Finish {
                    syntax,
                    kind,
                    origin,
                    roles,
                    base,
                } => {
                    assert_eq!(results.len() - base, roles.len());
                    let children = roles
                        .into_iter()
                        .zip(results.drain(base..))
                        .map(|(role, node)| Edge { role, node })
                        .collect();
                    let node = self
                        .mir
                        .node(self.module, self.location(syntax), kind, children);
                    self.mir.hir[node.index()].origin = Some(origin);
                    results.push(node);
                }
            }
        }
        assert_eq!(results.len(), 1);
        Ok(results[0])
    }

    fn shape(&self, node: NodeRef, mode: Mode) -> Result<Shape, Diagnostic> {
        match mode {
            Mode::Expression => self.expr(node),
            Mode::Name => Ok(Shape::Node(
                HirKind::Name(self.text(node).into_owned()),
                vec![],
            )),
            Mode::Type => Ok(Shape::Node(
                HirKind::TypeSyntax,
                vec![Input::with(Role::Operand, node, Mode::TypeTerm)],
            )),
            Mode::TypeTerm => self.type_term(node),
            Mode::TypeArgument => self.type_argument(node),
            Mode::Contract => self.contract(node),
            Mode::Path(last) => self.path(node, last),
            Mode::Parameter => self.parameter(node),
            Mode::ReturnSlot => self.return_slot(node),
            Mode::Body => self.body(node),
            Mode::Binding => self.binding(node),
            Mode::Discard => self.discard(node),
            Mode::DiscardName => Ok(Shape::Desugared(
                HirKind::Name(format!("\0discard_{}", self.location(node).start)),
                vec![],
            )),
            Mode::Unit => Ok(Shape::Desugared(HirKind::Tuple, vec![])),
        }
    }

    fn location(&self, node: NodeRef) -> Location {
        Location::from_usize(self.source, self.cst.span(node)).expect("registered CST span")
    }

    fn text(&self, node: NodeRef) -> std::borrow::Cow<'_, str> {
        self.mir
            .sources
            .get(self.source)
            .text()
            .slice(TextRange::from_usize(self.cst.span(node)).expect("registered CST span"))
            .expect("CST text")
    }

    fn error(&self, node: NodeRef, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(node))
    }

    fn rule(&self, node: NodeRef) -> Option<Rule> {
        SyntaxNode::new(self.cst, node).rule()
    }

    fn child(&self, node: NodeRef, rule: Rule) -> Result<NodeRef, Diagnostic> {
        self.cst
            .children(node)
            .find(|child| self.rule(*child) == Some(rule))
            .ok_or_else(|| self.error(node, format!("missing {rule:?}")))
    }

    fn token(&self, node: NodeRef, token: Token) -> Option<NodeRef> {
        self.cst
            .children(node)
            .find(|child| matches!(self.cst.get(*child), Node::Token(found, _) if found == token))
    }

    fn expressions(&self, node: NodeRef) -> Vec<NodeRef> {
        self.cst
            .children(node)
            .filter(|child| Expr::cast(self.cst, *child).is_some())
            .collect()
    }

    fn operands<const N: usize>(&self, node: NodeRef) -> Result<[NodeRef; N], Diagnostic> {
        self.expressions(node)
            .try_into()
            .map_err(|_| self.error(node, format!("expected {N} expression operands")))
    }
}

#[cfg(test)]
mod tests;
