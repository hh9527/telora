use super::lexer::Token;
use super::parser::{CstData, Node, NodeRef, Rule};
use crate::source::{Diagnostic, Location, SourceId, TextRange};

#[derive(Clone, Copy)]
pub struct SyntaxNode<'tree> {
    tree: &'tree CstData,
    node: NodeRef,
}

impl<'tree> SyntaxNode<'tree> {
    pub fn new(tree: &'tree CstData, node: NodeRef) -> Self {
        Self { tree, node }
    }

    pub fn node_ref(self) -> NodeRef {
        self.node
    }

    pub fn range(self) -> TextRange {
        TextRange::from_usize(self.tree.span(self.node)).expect("CST range fits registered source")
    }

    pub fn children(self) -> impl Iterator<Item = SyntaxNode<'tree>> {
        self.tree
            .children(self.node)
            .map(|node| SyntaxNode::new(self.tree, node))
    }

    pub fn rule(self) -> Option<Rule> {
        match self.tree.get(self.node) {
            Node::Rule(rule, _) => Some(rule),
            Node::Token(..) => None,
        }
    }

    pub fn token(self) -> Option<SyntaxToken<'tree>> {
        match self.tree.get(self.node) {
            Node::Token(kind, _) => Some(SyntaxToken { syntax: self, kind }),
            Node::Rule(..) => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SyntaxToken<'tree> {
    syntax: SyntaxNode<'tree>,
    kind: Token,
}

impl SyntaxToken<'_> {
    pub fn kind(self) -> Token {
        self.kind
    }

    pub fn range(self) -> TextRange {
        self.syntax.range()
    }
}

pub trait AstNode<'tree>: Sized + Copy {
    fn cast(tree: &'tree CstData, node: NodeRef) -> Option<Self>;
    fn syntax(self) -> SyntaxNode<'tree>;
}

#[derive(Clone, Copy)]
pub struct Program<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> Program<'tree> {
    pub fn root(tree: &'tree CstData) -> Self {
        Self::cast(tree, NodeRef::ROOT).expect("Lelwel root is a program")
    }

    pub fn body(self) -> Option<Body<'tree>> {
        child_node(self.syntax, Rule::Body).and_then(Body::from_syntax)
    }
}

impl<'tree> AstNode<'tree> for Program<'tree> {
    fn cast(tree: &'tree CstData, node: NodeRef) -> Option<Self> {
        let syntax = SyntaxNode::new(tree, node);
        (syntax.rule() == Some(Rule::Program)).then_some(Self { syntax })
    }

    fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }
}

#[derive(Clone, Copy)]
pub struct Body<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> Body<'tree> {
    fn from_syntax(syntax: SyntaxNode<'tree>) -> Option<Self> {
        (syntax.rule() == Some(Rule::Body)).then_some(Self { syntax })
    }

    pub fn bindings(self) -> impl Iterator<Item = Binding<'tree>> {
        self.syntax.children().filter_map(Binding::from_syntax)
    }

    pub fn result(self) -> Option<Expr<'tree>> {
        expression_slots(self.syntax).into_iter().last().flatten()
    }
}

impl<'tree> AstNode<'tree> for Body<'tree> {
    fn cast(tree: &'tree CstData, node: NodeRef) -> Option<Self> {
        Self::from_syntax(SyntaxNode::new(tree, node))
    }

    fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }
}

#[derive(Clone, Copy)]
pub enum Binding<'tree> {
    Let(LetBinding<'tree>),
    Decl(DeclBinding<'tree>),
    Def(DefBinding<'tree>),
    Native(NativeBinding<'tree>),
    NativeType(NativeTypeBinding<'tree>),
    Type(TypeBinding<'tree>),
    Trait(TraitBinding<'tree>),
    Impl(ImplBinding<'tree>),
    Import(ImportBinding<'tree>),
    Option(OptionBinding<'tree>),
    Export(ExportBinding<'tree>),
}

impl<'tree> Binding<'tree> {
    fn from_syntax(mut syntax: SyntaxNode<'tree>) -> Option<Self> {
        if syntax.rule() == Some(Rule::Binding) {
            syntax = syntax.children().find(|child| child.rule().is_some())?;
        }
        match syntax.rule()? {
            Rule::LetBinding => Some(Self::Let(LetBinding { syntax })),
            Rule::DeclBinding => Some(Self::Decl(DeclBinding { syntax })),
            Rule::DefBinding => Some(Self::Def(DefBinding { syntax })),
            Rule::NativeBinding => Some(Self::Native(NativeBinding { syntax })),
            Rule::NativeTypeBinding => Some(Self::NativeType(NativeTypeBinding { syntax })),
            Rule::TypeBinding => Some(Self::Type(TypeBinding { syntax })),
            Rule::TraitBinding => Some(Self::Trait(TraitBinding { syntax })),
            Rule::ImplBinding => Some(Self::Impl(ImplBinding { syntax })),
            Rule::ImportBinding => Some(Self::Import(ImportBinding { syntax })),
            Rule::OptionBinding => Some(Self::Option(OptionBinding { syntax })),
            Rule::ExportStatement => Some(Self::Export(ExportBinding { syntax })),
            _ => None,
        }
    }

    pub fn syntax(self) -> SyntaxNode<'tree> {
        match self {
            Self::Let(node) => node.syntax,
            Self::Decl(node) => node.syntax,
            Self::Def(node) => node.syntax,
            Self::Native(node) => node.syntax,
            Self::NativeType(node) => node.syntax,
            Self::Type(node) => node.syntax,
            Self::Trait(node) => node.syntax,
            Self::Impl(node) => node.syntax,
            Self::Import(node) => node.syntax,
            Self::Option(node) => node.syntax,
            Self::Export(node) => node.syntax,
        }
    }

    pub fn name(self) -> Option<SyntaxToken<'tree>> {
        match self {
            Self::Let(node) => node.name(),
            Self::Decl(node) => node.name(),
            Self::Def(node) => node.name(),
            Self::Native(node) => node.name(),
            Self::NativeType(node) => node.name(),
            Self::Type(node) => node.name(),
            Self::Trait(node) => node.name(),
            Self::Impl(_) => None,
            Self::Import(node) => node.name(),
            Self::Option(_) => None,
            Self::Export(_) => None,
        }
    }
}

macro_rules! binding_node {
    ($name:ident) => {
        #[derive(Clone, Copy)]
        pub struct $name<'tree> {
            syntax: SyntaxNode<'tree>,
        }

        impl<'tree> $name<'tree> {
            pub fn syntax(self) -> SyntaxNode<'tree> {
                self.syntax
            }

            pub fn name(self) -> Option<SyntaxToken<'tree>> {
                token_child(self.syntax, Token::Identifier)
            }
        }
    };
}

binding_node!(LetBinding);
binding_node!(DeclBinding);
binding_node!(DefBinding);
binding_node!(NativeBinding);
binding_node!(NativeTypeBinding);
binding_node!(TypeBinding);
binding_node!(TraitBinding);
binding_node!(ImplBinding);
binding_node!(ImportBinding);
binding_node!(OptionBinding);
binding_node!(ExportBinding);

#[derive(Clone, Copy)]
pub struct Decorator<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> Decorator<'tree> {
    pub fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }

    pub fn path(self) -> Option<SyntaxNode<'tree>> {
        child_node(self.syntax, Rule::DecoratorPath)
    }

    pub fn arguments(self) -> Option<SyntaxNode<'tree>> {
        child_node(self.syntax, Rule::Arguments)
    }
}

impl<'tree> LetBinding<'tree> {
    pub fn annotation(self) -> Option<Expr<'tree>> {
        token_child(self.syntax, Token::Colon)?;
        expression_slots(self.syntax).first().copied().flatten()
    }

    pub fn value(self) -> Option<Expr<'tree>> {
        let slots = expression_slots(self.syntax);
        let index = usize::from(token_child(self.syntax, Token::Colon).is_some());
        slots.get(index).copied().flatten()
    }

    fn value_slot(self) -> Option<SyntaxNode<'tree>> {
        let slots = expression_slot_nodes(self.syntax);
        let index = usize::from(token_child(self.syntax, Token::Colon).is_some());
        slots.get(index).copied()
    }
}

impl<'tree> DeclBinding<'tree> {
    pub fn type_parameters(self) -> Option<SyntaxNode<'tree>> {
        child_node(self.syntax, Rule::TypeScheme)
            .and_then(|scheme| child_node(scheme, Rule::TypeParameters))
    }

    pub fn contract(self) -> Option<SyntaxNode<'tree>> {
        contract_in_type_scheme(self.syntax)
    }
}

impl<'tree> NativeBinding<'tree> {
    pub fn type_parameters(self) -> Option<SyntaxNode<'tree>> {
        child_node(self.syntax, Rule::TypeScheme)
            .and_then(|scheme| child_node(scheme, Rule::TypeParameters))
    }

    pub fn contract(self) -> Option<SyntaxNode<'tree>> {
        contract_in_type_scheme(self.syntax)
    }
}

fn contract_in_type_scheme(binding: SyntaxNode<'_>) -> Option<SyntaxNode<'_>> {
    child_node(binding, Rule::TypeScheme)?
        .children()
        .find(|child| {
            matches!(
                child.rule(),
                Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
            )
        })
}

impl<'tree> DefBinding<'tree> {
    pub fn type_parameters(self) -> Option<SyntaxNode<'tree>> {
        child_node(self.syntax, Rule::TypeScheme)
            .and_then(|scheme| child_node(scheme, Rule::TypeParameters))
    }

    pub fn contract(self) -> Option<SyntaxNode<'tree>> {
        contract_in_type_scheme(self.syntax)
    }

    pub fn value(self) -> Option<Expr<'tree>> {
        expression_slots(self.syntax).first().copied().flatten()
    }

    fn value_slot(self) -> Option<SyntaxNode<'tree>> {
        expression_slot_nodes(self.syntax).first().copied()
    }
}

impl<'tree> TypeBinding<'tree> {
    pub fn decorators(self) -> impl Iterator<Item = Decorator<'tree>> {
        self.syntax.children().filter_map(|syntax| {
            (syntax.rule() == Some(Rule::Decorator)).then_some(Decorator { syntax })
        })
    }

    pub fn type_parameters(self) -> Option<SyntaxNode<'tree>> {
        child_node(self.syntax, Rule::TypeParameters)
    }

    pub fn value(self) -> Option<Expr<'tree>> {
        expression_slots(self.syntax).first().copied().flatten()
    }

    pub fn initializer(self) -> Option<TypeInitializer<'tree>> {
        self.syntax
            .children()
            .find_map(TypeInitializer::from_syntax)
    }

    fn value_slot(self) -> Option<SyntaxNode<'tree>> {
        expression_slot_nodes(self.syntax)
            .first()
            .copied()
            .or_else(|| self.initializer().map(TypeInitializer::syntax))
    }
}

#[derive(Clone, Copy)]
pub enum TypeInitializer<'tree> {
    Struct(StructInitializer<'tree>),
    Enum(EnumInitializer<'tree>),
}

impl<'tree> TypeInitializer<'tree> {
    fn from_syntax(syntax: SyntaxNode<'tree>) -> Option<Self> {
        match syntax.rule()? {
            Rule::StructInitializer => Some(Self::Struct(StructInitializer { syntax })),
            Rule::EnumInitializer => Some(Self::Enum(EnumInitializer { syntax })),
            _ => None,
        }
    }

    pub fn syntax(self) -> SyntaxNode<'tree> {
        match self {
            Self::Struct(initializer) => initializer.syntax,
            Self::Enum(initializer) => initializer.syntax,
        }
    }
}

#[derive(Clone, Copy)]
pub struct StructInitializer<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> StructInitializer<'tree> {
    pub fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }

    pub fn fields(self) -> impl Iterator<Item = SyntaxNode<'tree>> {
        self.syntax
            .children()
            .filter(|child| child.rule() == Some(Rule::StructInitializerField))
    }
}

#[derive(Clone, Copy)]
pub struct EnumInitializer<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> EnumInitializer<'tree> {
    pub fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }

    pub fn variants(self) -> impl Iterator<Item = SyntaxNode<'tree>> {
        self.syntax
            .children()
            .filter(|child| child.rule() == Some(Rule::EnumInitializerVariant))
    }
}

impl<'tree> ImportBinding<'tree> {
    pub fn path(self) -> Option<StringLiteral<'tree>> {
        child_node(self.syntax, Rule::StringLiteral).map(|syntax| StringLiteral { syntax })
    }

    pub fn has_items(self) -> bool {
        child_node(self.syntax, Rule::ImportSelector)
            .and_then(|selector| child_node(selector, Rule::ImportItems))
            .is_some()
    }

    pub fn has_selector(self) -> bool {
        child_node(self.syntax, Rule::ImportSelector).is_some()
    }
}

#[derive(Clone, Copy)]
pub struct Expr<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> Expr<'tree> {
    fn from_syntax(syntax: SyntaxNode<'tree>) -> Option<Self> {
        is_complete_expression(syntax).then_some(Self { syntax })
    }
}

impl<'tree> AstNode<'tree> for Expr<'tree> {
    fn cast(tree: &'tree CstData, node: NodeRef) -> Option<Self> {
        Self::from_syntax(SyntaxNode::new(tree, node))
    }

    fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }
}

#[derive(Clone, Copy)]
pub struct StringLiteral<'tree> {
    syntax: SyntaxNode<'tree>,
}

impl<'tree> StringLiteral<'tree> {
    pub fn parts(self) -> impl Iterator<Item = SyntaxNode<'tree>> {
        self.syntax.children().filter(|child| {
            matches!(
                child.token().map(SyntaxToken::kind),
                Some(
                    Token::StringText
                        | Token::EscapeSequence
                        | Token::UnknownEscapeSequence
                        | Token::UnterminatedEscapeSequence
                        | Token::RawString
                )
            ) || child.rule() == Some(Rule::Interpolation)
        })
    }
}

impl<'tree> AstNode<'tree> for StringLiteral<'tree> {
    fn cast(tree: &'tree CstData, node: NodeRef) -> Option<Self> {
        let syntax = SyntaxNode::new(tree, node);
        (syntax.rule() == Some(Rule::StringLiteral)).then_some(Self { syntax })
    }

    fn syntax(self) -> SyntaxNode<'tree> {
        self.syntax
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedSyntax {
    ProgramBody,
    BindingName,
    BindingValue,
    BindingContract,
    ImportPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxIssue {
    pub location: Location,
    pub expected: ExpectedSyntax,
}

impl SyntaxIssue {
    pub fn into_diagnostic(self) -> Diagnostic {
        Diagnostic::error(
            format!("missing {}", expected_name(&self.expected)),
            self.location,
        )
    }
}

pub fn validate(source: SourceId, tree: &CstData) -> Vec<SyntaxIssue> {
    let program = Program::root(tree);
    let Some(body) = program.body() else {
        return vec![missing_at(
            source,
            program.syntax(),
            ExpectedSyntax::ProgramBody,
        )];
    };
    let mut issues = Vec::new();
    for binding in body.bindings() {
        if binding.name().is_none()
            && !matches!(binding, Binding::Import(import) if import.has_selector())
            && !matches!(
                binding,
                Binding::Option(_) | Binding::Export(_) | Binding::Impl(_)
            )
        {
            issues.push(missing_after_keyword(source, binding));
        }
        match binding {
            Binding::Let(node) if node.value().is_none() => issues.push(missing_slot(
                source,
                node.value_slot(),
                node.syntax,
                Some(Token::Semicolon),
                ExpectedSyntax::BindingValue,
            )),
            Binding::Decl(node) if node.contract().is_none() => issues.push(missing_at(
                source,
                node.syntax,
                ExpectedSyntax::BindingContract,
            )),
            Binding::Def(node) if node.value().is_none() => issues.push(missing_slot(
                source,
                node.value_slot(),
                node.syntax,
                Some(Token::Semicolon),
                ExpectedSyntax::BindingValue,
            )),
            Binding::Type(node) if node.value().is_none() && node.initializer().is_none() => issues
                .push(missing_slot(
                    source,
                    node.value_slot(),
                    node.syntax,
                    Some(Token::Semicolon),
                    ExpectedSyntax::BindingValue,
                )),
            Binding::Import(node) if node.path().is_none() => {
                issues.push(missing_at(source, node.syntax, ExpectedSyntax::ImportPath))
            }
            Binding::Export(_) => {}
            _ => {}
        }
    }
    issues
}

fn child_node(syntax: SyntaxNode<'_>, rule: Rule) -> Option<SyntaxNode<'_>> {
    syntax.children().find(|child| child.rule() == Some(rule))
}

fn token_child(syntax: SyntaxNode<'_>, kind: Token) -> Option<SyntaxToken<'_>> {
    syntax
        .children()
        .filter_map(SyntaxNode::token)
        .find(|token| token.kind() == kind)
}

fn expression_slot_nodes(syntax: SyntaxNode<'_>) -> Vec<SyntaxNode<'_>> {
    syntax
        .children()
        .filter(|child| is_expression_slot(*child))
        .collect()
}

fn expression_slots(syntax: SyntaxNode<'_>) -> Vec<Option<Expr<'_>>> {
    expression_slot_nodes(syntax)
        .into_iter()
        .map(Expr::from_syntax)
        .collect()
}

fn is_expression_slot(syntax: SyntaxNode<'_>) -> bool {
    matches!(
        syntax.rule(),
        Some(
            Rule::Expression
                | Rule::Primary
                | Rule::Braced
                | Rule::ArrayExpr
                | Rule::AtomExpr
                | Rule::BinaryExpr
                | Rule::Block
                | Rule::BytesExpr
                | Rule::CallExpr
                | Rule::Closure
                | Rule::FunctionContract
                | Rule::DictExpr
                | Rule::DoExpr
                | Rule::IndexExpr
                | Rule::FloatExpr
                | Rule::IfExpr
                | Rule::IfLetExpr
                | Rule::InterpreterIntrinsic
                | Rule::NamedIntrinsic
                | Rule::LegacyInterpreterExpr
                | Rule::IntExpr
                | Rule::MatchExpr
                | Rule::ParenExpr
                | Rule::PipelineExpr
                | Rule::DotPostfixExpr
                | Rule::PropagateExpr
                | Rule::ReturnExpr
                | Rule::SectionExpr
                | Rule::StringExpr
                | Rule::TypeApplyExpr
                | Rule::UnaryExpr
                | Rule::VariableExpr
        )
    )
}

fn is_complete_expression(syntax: SyntaxNode<'_>) -> bool {
    if !is_expression_slot(syntax) {
        return false;
    }
    match syntax.rule() {
        Some(Rule::Expression | Rule::Primary | Rule::Braced) => {
            syntax.children().any(is_complete_expression)
        }
        Some(_) => true,
        None => false,
    }
}

fn missing_after_keyword(source: SourceId, binding: Binding<'_>) -> SyntaxIssue {
    let keyword = match binding {
        Binding::Let(_) => Token::Let,
        Binding::Decl(_) => Token::Decl,
        Binding::Def(_) => Token::Def,
        Binding::Native(_) => Token::Native,
        Binding::NativeType(_) => Token::Native,
        Binding::Type(_) => Token::Type,
        Binding::Trait(_) => Token::Trait,
        Binding::Impl(_) => Token::Impl,
        Binding::Import(_) => Token::Import,
        Binding::Option(_) => Token::Option,
        Binding::Export(_) => Token::Export,
    };
    let syntax = binding.syntax();
    let mut found_keyword = false;
    let mut offset = syntax.range().start;
    for child in syntax.children() {
        let Some(token) = child.token() else {
            continue;
        };
        if token.kind() == keyword {
            found_keyword = true;
            offset = token.range().end;
        } else if found_keyword && !matches!(token.kind(), Token::Whitespace | Token::Comment) {
            offset = token.range().start;
            break;
        }
    }
    SyntaxIssue {
        location: Location::new(source, TextRange::at(offset)),
        expected: ExpectedSyntax::BindingName,
    }
}

fn missing_slot(
    source: SourceId,
    slot: Option<SyntaxNode<'_>>,
    parent: SyntaxNode<'_>,
    before: Option<Token>,
    expected: ExpectedSyntax,
) -> SyntaxIssue {
    let range = slot.map_or_else(
        || {
            before
                .and_then(|kind| token_child(parent, kind))
                .map_or_else(|| TextRange::at(parent.range().end), SyntaxToken::range)
        },
        SyntaxNode::range,
    );
    SyntaxIssue {
        location: Location::new(source, TextRange::at(range.start)),
        expected,
    }
}

fn missing_at(source: SourceId, parent: SyntaxNode<'_>, expected: ExpectedSyntax) -> SyntaxIssue {
    SyntaxIssue {
        location: Location::new(source, TextRange::at(parent.range().end)),
        expected,
    }
}

fn expected_name(expected: &ExpectedSyntax) -> &'static str {
    match expected {
        ExpectedSyntax::ProgramBody => "program body",
        ExpectedSyntax::BindingName => "binding name",
        ExpectedSyntax::BindingValue => "binding value",
        ExpectedSyntax::BindingContract => "binding contract",
        ExpectedSyntax::ImportPath => "import path",
    }
}
