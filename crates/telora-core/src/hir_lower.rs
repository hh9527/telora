//! CST-backed semantic lowering into the session's flat HIR arena.
//!
//! Each task contains syntax references and shallow semantic state, not a tree.

use crate::mir::{Edge, HirId, HirKind, HirOrigin, Mir, ModuleId, Role};
use crate::source::{Diagnostic, Location, SourceId, TextRange};
use crate::syntax::telora::{
    ast::{AstNode, Expr, SyntaxNode},
    lexer::Token,
    parser::{CstData, Node, NodeRef, Rule},
};
use std::cell::{Cell, RefCell};

mod contracts;
mod control;
mod declarations;
mod expressions;
mod imports;
mod intrinsics;
mod literals;
mod modules;
mod patterns;
mod records;
mod scopes;
mod sections;
mod types;
pub use modules::lower_module;

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
        plans: RefCell::new(Vec::new()),
        needs_display: Cell::new(false),
        errors: RefCell::new(vec![]),
    }
    .expression_result(root)
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
    Synthetic(usize),
    Pattern,
    Arm,
    PatternField,
    DictField,
    Decorator,
    Module,
    TypeParameter,
    TypeInitializer,
    TypeMember,
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
    // Shallow expansion plans, consumed once. No plan owns another plan.
    plans: RefCell<Vec<Option<Shape>>>,
    needs_display: Cell<bool>,
    errors: RefCell<Vec<Diagnostic>>,
}

impl Lower<'_> {
    fn expression_result(&mut self, root: NodeRef) -> Result<HirId, Diagnostic> {
        let node = self.run(root, Mode::Expression);
        let errors = self.errors.take();
        if let Some(error) = errors.into_iter().next() {
            Err(error)
        } else {
            Ok(node)
        }
    }

    fn run(&mut self, root: NodeRef, mode: Mode) -> HirId {
        let mut tasks = vec![Task::Visit(root, mode)];
        let mut results = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(syntax, mode) => {
                    let shape = match self.shape(syntax, mode) {
                        Ok(shape) => shape,
                        Err(()) => Shape::Node(HirKind::Missing, vec![]),
                    };
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
                    let mut children: Vec<Edge> = roles
                        .into_iter()
                        .zip(results.drain(base..))
                        .map(|(role, node)| Edge { role, node })
                        .collect();
                    if matches!(kind, HirKind::Block) {
                        for edge in &mut children {
                            if edge.role == Role::Binding
                                && matches!(self.mir.hir[edge.node.index()].kind, HirKind::Missing)
                            {
                                edge.role = Role::Value;
                            }
                        }
                    }
                    let node = self
                        .mir
                        .node(self.module, self.location(syntax), kind, children);
                    self.mir.hir[node.index()].origin = Some(origin);
                    results.push(node);
                }
            }
        }
        assert_eq!(results.len(), 1);
        results[0]
    }

    fn shape(&self, node: NodeRef, mode: Mode) -> Result<Shape, ()> {
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
            Mode::Synthetic(id) => Ok(self.plans.borrow_mut()[id]
                .take()
                .expect("unique expansion plan")),
            Mode::Pattern => self.pattern(node),
            Mode::Arm => self.arm(node),
            Mode::PatternField => self.pattern_field(node),
            Mode::DictField => self.dict_field(node),
            Mode::Decorator => self.decorator(node),
            Mode::Module => self.module_body(node),
            Mode::TypeParameter => self.type_parameter(node),
            Mode::TypeInitializer => self.type_initializer(node),
            Mode::TypeMember => self.type_member(node),
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

    fn error(&self, node: NodeRef, message: impl Into<String>) {
        self.errors
            .borrow_mut()
            .push(Diagnostic::error(message, self.location(node)));
    }

    fn rule(&self, node: NodeRef) -> Option<Rule> {
        SyntaxNode::new(self.cst, node).rule()
    }

    fn child(&self, node: NodeRef, rule: Rule) -> Result<NodeRef, ()> {
        self.cst
            .children(node)
            .find(|child| self.rule(*child) == Some(rule))
            .ok_or(())
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

    fn operands<const N: usize>(&self, node: NodeRef) -> Result<[NodeRef; N], ()> {
        self.expressions(node).try_into().map_err(|_| ())
    }

    fn synthetic(&self, role: Role, node: NodeRef, kind: HirKind, inputs: Vec<Input>) -> Input {
        let mut plans = self.plans.borrow_mut();
        let id = plans.len();
        plans.push(Some(Shape::Desugared(kind, inputs)));
        Input::with(role, node, Mode::Synthetic(id))
    }

    fn optional_expression(&self, role: Role, parent: NodeRef, node: Option<NodeRef>) -> Input {
        match node {
            Some(node) => Input::expr(role, node),
            None => self.synthetic(role, parent, HirKind::Missing, vec![]),
        }
    }

    fn required_token(&self, node: NodeRef, token: Token) -> Result<NodeRef, ()> {
        self.token(node, token).ok_or(())
    }

    fn first_expression(&self, node: NodeRef) -> Result<NodeRef, ()> {
        self.expressions(node).into_iter().next().ok_or(())
    }
}

#[cfg(test)]
mod tests;
