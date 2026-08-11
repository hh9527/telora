use crate::ast::{
    Binding, BindingKind, Block, Expr, ExprKind, MatchArm, Pattern, PatternKind, Program,
    StringPartKind, TypeArgumentKind,
};
use crate::parser::RecoveredProgram;
use crate::source::Location;
use std::collections::{HashMap, HashSet};

macro_rules! hir_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u32);

        impl $name {
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

hir_id!(HirDefinitionId);
hir_id!(HirReferenceId);
hir_id!(HirExpressionId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirDefinitionKind {
    Let,
    DefinitionSlot,
    Type,
    Import,
    Native,
    Parameter,
    Pattern,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirResolution {
    Definition(HirDefinitionId),
    External,
    Unresolved,
}

#[derive(Clone, Debug)]
pub struct HirTypeParameter {
    pub name: String,
    pub location: Location,
}

#[derive(Clone, Debug)]
pub struct HirDefinition {
    pub id: HirDefinitionId,
    pub name: String,
    pub kind: HirDefinitionKind,
    pub location: Location,
    pub additional_locations: Vec<Location>,
    pub type_parameters: Vec<HirTypeParameter>,
    pub top_level: bool,
    pub value: Option<HirExpressionId>,
}

#[derive(Clone, Debug)]
pub struct HirReference {
    pub id: HirReferenceId,
    pub name: String,
    pub location: Location,
    pub resolution: HirResolution,
}

#[derive(Clone, Debug)]
pub struct HirExpression {
    pub id: HirExpressionId,
    pub location: Location,
    pub parent: Option<HirExpressionId>,
    pub reference: Option<HirReferenceId>,
}

#[derive(Clone, Debug, Default)]
pub struct HirProgram {
    definitions: Vec<HirDefinition>,
    references: Vec<HirReference>,
    expressions: Vec<HirExpression>,
}

impl HirProgram {
    pub fn resolve(program: &Program, external_names: impl IntoIterator<Item = String>) -> Self {
        let mut resolver = Resolver {
            hir: Self::default(),
            external_names: external_names.into_iter().collect(),
            expression_stack: Vec::new(),
        };
        let mut scopes = Vec::new();
        resolver.index_block(&program.value.body, &mut scopes, true);
        resolver.hir.normalize_order();
        resolver.hir
    }

    pub fn resolve_expression(
        expression: &Expr,
        external_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut resolver = Resolver {
            hir: Self::default(),
            external_names: external_names.into_iter().collect(),
            expression_stack: Vec::new(),
        };
        resolver.index_expr(expression, &mut Vec::new());
        resolver.hir.normalize_order();
        resolver.hir
    }

    pub fn resolve_recovered(
        program: &RecoveredProgram,
        external_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut resolver = Resolver {
            hir: Self::default(),
            external_names: external_names.into_iter().collect(),
            expression_stack: Vec::new(),
        };
        resolver.index_block_parts(
            &program.bindings,
            program.result.as_ref(),
            &mut Vec::new(),
            true,
        );
        resolver.hir.normalize_order();
        resolver.hir
    }

    pub fn definitions(&self) -> &[HirDefinition] {
        &self.definitions
    }

    pub fn definition(&self, id: HirDefinitionId) -> Option<&HirDefinition> {
        self.definitions.get(id.index())
    }

    pub fn references(&self) -> &[HirReference] {
        &self.references
    }

    pub fn reference(&self, id: HirReferenceId) -> Option<&HirReference> {
        self.references.get(id.index())
    }

    pub fn expressions(&self) -> &[HirExpression] {
        &self.expressions
    }

    pub fn expression(&self, id: HirExpressionId) -> Option<&HirExpression> {
        self.expressions.get(id.index())
    }

    pub fn expression_ids_at(&self, location: Location) -> impl Iterator<Item = HirExpressionId> {
        self.expressions
            .iter()
            .filter(move |expression| expression.location == location)
            .map(|expression| expression.id)
    }

    pub fn unresolved(&self) -> impl Iterator<Item = &HirReference> {
        self.references
            .iter()
            .filter(|reference| reference.resolution == HirResolution::Unresolved)
    }

    fn normalize_order(&mut self) {
        self.definitions.sort_by_key(|definition| {
            (
                definition.location.source,
                definition.location.start,
                definition.location.end,
            )
        });
        let mut definitions = vec![HirDefinitionId(0); self.definitions.len()];
        for (index, definition) in self.definitions.iter_mut().enumerate() {
            let old = definition.id;
            let new = HirDefinitionId(index as u32);
            definition.id = new;
            definitions[old.index()] = new;
        }
        for reference in &mut self.references {
            if let HirResolution::Definition(definition) = &mut reference.resolution {
                *definition = definitions[definition.index()];
            }
        }

        self.references.sort_by_key(|reference| {
            (
                reference.location.source,
                reference.location.start,
                reference.location.end,
            )
        });
        let mut references = vec![HirReferenceId(0); self.references.len()];
        for (index, reference) in self.references.iter_mut().enumerate() {
            let old = reference.id;
            let new = HirReferenceId(index as u32);
            reference.id = new;
            references[old.index()] = new;
        }
        for expression in &mut self.expressions {
            expression.reference = expression.reference.map(|id| references[id.index()]);
        }

        self.expressions.sort_by_key(|expression| {
            (
                expression.location.source,
                expression.location.start,
                expression.location.end,
            )
        });
        let mut expressions = vec![HirExpressionId(0); self.expressions.len()];
        for (index, expression) in self.expressions.iter_mut().enumerate() {
            let old = expression.id;
            expression.id = HirExpressionId(index as u32);
            expressions[old.index()] = expression.id;
        }
        for definition in &mut self.definitions {
            definition.value = definition.value.map(|id| expressions[id.index()]);
        }
        for expression in &mut self.expressions {
            expression.parent = expression.parent.map(|id| expressions[id.index()]);
        }
    }
}

type Scope = HashMap<String, HirDefinitionId>;

struct Resolver {
    hir: HirProgram,
    external_names: HashSet<String>,
    expression_stack: Vec<HirExpressionId>,
}

impl Resolver {
    fn define(&mut self, binding: &Binding, scope: &mut Scope, top_level: bool) -> HirDefinitionId {
        let name = binding.value.name.value.as_str();
        let kind = match binding.value.kind {
            BindingKind::Let => HirDefinitionKind::Let,
            BindingKind::Decl | BindingKind::Def => HirDefinitionKind::DefinitionSlot,
            BindingKind::Type => HirDefinitionKind::Type,
            BindingKind::Import => HirDefinitionKind::Import,
            BindingKind::OpenImport => HirDefinitionKind::Import,
            BindingKind::Export => HirDefinitionKind::Import,
            BindingKind::Native | BindingKind::NativeType => HirDefinitionKind::Native,
        };
        let id = self.define_name(name, kind, binding.value.name.location, scope, top_level);
        self.hir.definitions[id.index()].type_parameters = binding
            .value
            .type_parameters
            .iter()
            .map(|parameter| HirTypeParameter {
                name: parameter.value.clone(),
                location: parameter.location,
            })
            .collect();
        id
    }

    fn define_name(
        &mut self,
        name: &str,
        kind: HirDefinitionKind,
        location: Location,
        scope: &mut Scope,
        top_level: bool,
    ) -> HirDefinitionId {
        let id = HirDefinitionId(self.hir.definitions.len() as u32);
        self.hir.definitions.push(HirDefinition {
            id,
            name: name.into(),
            kind,
            location,
            additional_locations: Vec::new(),
            type_parameters: Vec::new(),
            top_level,
            value: None,
        });
        scope.insert(name.into(), id);
        id
    }

    fn index_block(&mut self, block: &Block, scopes: &mut Vec<Scope>, top_level: bool) {
        self.index_block_parts(
            &block.value.bindings,
            Some(&block.value.result),
            scopes,
            top_level,
        );
    }

    fn index_block_parts(
        &mut self,
        bindings: &[Binding],
        result: Option<&Expr>,
        scopes: &mut Vec<Scope>,
        top_level: bool,
    ) {
        scopes.push(Scope::new());
        for binding in bindings {
            if matches!(
                binding.value.kind,
                BindingKind::Decl
                    | BindingKind::Native
                    | BindingKind::NativeType
                    | BindingKind::Type
            ) || binding.value.kind == BindingKind::Def && binding.value.annotation.is_some()
                || binding.value.kind == BindingKind::Def
                    && matches!(binding.value.value.value, ExprKind::Closure { .. })
                    && resolve_name(scopes, &binding.value.name.value).is_none()
            {
                self.define(
                    binding,
                    scopes.last_mut().expect("block has a scope"),
                    top_level,
                );
            }
        }
        for binding in bindings {
            if matches!(
                binding.value.kind,
                BindingKind::OpenImport | BindingKind::Export
            ) {
                continue;
            }
            if let Some(annotation) = &binding.value.annotation {
                self.index_binding_expr(binding, annotation, scopes);
            }
            match binding.value.kind {
                BindingKind::Let | BindingKind::Import => {
                    let value = self.index_expr(&binding.value.value, scopes);
                    let definition = self.define(
                        binding,
                        scopes.last_mut().expect("block has a scope"),
                        top_level,
                    );
                    self.hir.definitions[definition.index()].value = Some(value);
                }
                BindingKind::Def => {
                    let definition =
                        if let Some(id) = resolve_name(scopes, &binding.value.name.value) {
                            if binding.value.annotation.is_none()
                                && self.hir.definitions[id.index()].location
                                    != binding.value.name.location
                            {
                                self.hir.definitions[id.index()]
                                    .additional_locations
                                    .push(binding.value.name.location);
                            }
                            id
                        } else {
                            self.define(
                                binding,
                                scopes.last_mut().expect("block has a scope"),
                                top_level,
                            )
                        };
                    let value = if binding.value.annotation.is_some() {
                        self.index_binding_expr(binding, &binding.value.value, scopes)
                    } else {
                        self.index_expr(&binding.value.value, scopes)
                    };
                    self.hir.definitions[definition.index()].value = Some(value);
                }
                BindingKind::Decl
                | BindingKind::Native
                | BindingKind::NativeType
                | BindingKind::Type => {
                    let value = self.index_binding_expr(binding, &binding.value.value, scopes);
                    let definition = resolve_name(scopes, &binding.value.name.value)
                        .expect("predeclared binding has a definition");
                    self.hir.definitions[definition.index()].value = Some(value);
                }
                BindingKind::OpenImport => unreachable!("open imports are dependency edges"),
                BindingKind::Export => unreachable!("exports are module interface edges"),
            }
        }
        if let Some(result) = result {
            self.index_expr(result, scopes);
        }
        scopes.pop();
    }

    fn index_binding_expr(
        &mut self,
        binding: &Binding,
        expression: &Expr,
        scopes: &mut Vec<Scope>,
    ) -> HirExpressionId {
        let inserted = binding
            .value
            .type_parameters
            .iter()
            .map(|parameter| self.external_names.insert(parameter.value.clone()))
            .collect::<Vec<_>>();
        let expression = self.index_expr(expression, scopes);
        for (parameter, inserted) in binding.value.type_parameters.iter().zip(inserted) {
            if inserted {
                self.external_names.remove(&parameter.value);
            }
        }
        expression
    }

    fn index_expr(&mut self, expression: &Expr, scopes: &mut Vec<Scope>) -> HirExpressionId {
        let expression_id = HirExpressionId(self.hir.expressions.len() as u32);
        self.hir.expressions.push(HirExpression {
            id: expression_id,
            location: expression.location,
            parent: self.expression_stack.last().copied(),
            reference: None,
        });
        self.expression_stack.push(expression_id);
        let reference = match &expression.value {
            ExprKind::Variable(name) => {
                let resolution = resolve_name(scopes, &name.value).map_or_else(
                    || {
                        if self.external_names.contains(&name.value) {
                            HirResolution::External
                        } else {
                            HirResolution::Unresolved
                        }
                    },
                    HirResolution::Definition,
                );
                let id = HirReferenceId(self.hir.references.len() as u32);
                self.hir.references.push(HirReference {
                    id,
                    name: name.value.clone(),
                    location: name.location,
                    resolution,
                });
                Some(id)
            }
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(expression) = &part.value {
                        self.index_expr(expression, scopes);
                    }
                }
                None
            }
            ExprKind::Array(items) | ExprKind::Tuple(items) => {
                for item in items {
                    self.index_expr(item, scopes);
                }
                None
            }
            ExprKind::Spread(operand) => {
                self.index_expr(operand, scopes);
                None
            }
            ExprKind::Dict(fields) => {
                for field in fields {
                    self.index_expr(&field.value.value, scopes);
                }
                None
            }
            ExprKind::Block(block) => {
                self.index_block(block, scopes, false);
                None
            }
            ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
                self.index_expr(operand, scopes);
                None
            }
            ExprKind::Return { value } => {
                self.index_expr(value, scopes);
                None
            }
            ExprKind::Panic { message } => {
                self.index_expr(message, scopes);
                None
            }
            ExprKind::Raise { error } => {
                self.index_expr(error, scopes);
                None
            }
            ExprKind::Binary { left, right, .. } => {
                self.index_expr(left, scopes);
                self.index_expr(right, scopes);
                None
            }
            ExprKind::Field { receiver, .. } => {
                self.index_expr(receiver, scopes);
                None
            }
            ExprKind::Call { callee, arguments } => {
                self.index_expr(callee, scopes);
                for argument in arguments {
                    self.index_expr(argument, scopes);
                }
                None
            }
            ExprKind::TypeApply { callee, arguments } => {
                self.index_expr(callee, scopes);
                for argument in arguments {
                    match &argument.value {
                        TypeArgumentKind::Explicit(argument) => {
                            self.index_expr(argument, scopes);
                        }
                        TypeArgumentKind::Infer => {
                            let id = HirExpressionId(self.hir.expressions.len() as u32);
                            self.hir.expressions.push(HirExpression {
                                id,
                                location: argument.location,
                                parent: self.expression_stack.last().copied(),
                                reference: None,
                            });
                        }
                    }
                }
                None
            }
            ExprKind::Interpreter { operand, .. } => {
                self.index_expr(operand, scopes);
                None
            }
            ExprKind::Closure {
                parameters,
                result_annotation,
                body,
            } => {
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        self.index_expr(annotation, scopes);
                    }
                }
                if let Some(annotation) = result_annotation {
                    self.index_expr(annotation, scopes);
                }
                scopes.push(Scope::new());
                for parameter in parameters {
                    self.define_name(
                        &parameter.name.value,
                        HirDefinitionKind::Parameter,
                        parameter.name.location,
                        scopes.last_mut().expect("closure has a scope"),
                        false,
                    );
                }
                self.index_block(body, scopes, false);
                scopes.pop();
                None
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.index_expr(condition, scopes);
                self.index_block(then_branch, scopes, false);
                self.index_block(else_branch, scopes, false);
                None
            }
            ExprKind::IfLet {
                pattern,
                value,
                then_branch,
                else_branch,
            } => {
                self.index_expr(value, scopes);
                scopes.push(Scope::new());
                self.index_pattern(pattern, scopes.last_mut().expect("if let has a scope"));
                self.index_block(then_branch, scopes, false);
                scopes.pop();
                self.index_block(else_branch, scopes, false);
                None
            }
            ExprKind::LetElse {
                pattern,
                value,
                else_branch,
                body,
            } => {
                self.index_expr(value, scopes);
                self.index_block(else_branch, scopes, false);
                scopes.push(Scope::new());
                self.index_pattern(pattern, scopes.last_mut().expect("let else has a scope"));
                self.index_block(body, scopes, false);
                scopes.pop();
                None
            }
            ExprKind::Match { value, arms } => {
                self.index_expr(value, scopes);
                for arm in arms {
                    self.index_arm(arm, scopes);
                }
                None
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bytes(_)
            | ExprKind::Atom(_) => None,
        };
        self.expression_stack.pop();
        self.hir.expressions[expression_id.index()].reference = reference;
        expression_id
    }

    fn index_arm(&mut self, arm: &MatchArm, scopes: &mut Vec<Scope>) {
        scopes.push(Scope::new());
        self.index_pattern(
            &arm.value.pattern,
            scopes.last_mut().expect("arm has a scope"),
        );
        if let Some(guard) = &arm.value.guard {
            self.index_expr(guard, scopes);
        }
        self.index_expr(&arm.value.value, scopes);
        scopes.pop();
    }

    fn index_pattern(&mut self, pattern: &Pattern, scope: &mut Scope) {
        match &pattern.value {
            PatternKind::Binding(name) => {
                self.define_name(
                    &name.value,
                    HirDefinitionKind::Pattern,
                    name.location,
                    scope,
                    false,
                );
            }
            PatternKind::Tuple(items) => {
                for item in items {
                    self.index_pattern(item, scope);
                }
            }
            PatternKind::Tagged { payload, .. } => self.index_pattern(payload, scope),
            PatternKind::Struct(fields) => {
                for field in fields {
                    self.index_pattern(&field.pattern, scope);
                }
            }
            _ => {}
        }
    }
}

fn resolve_name(scopes: &[Scope], name: &str) -> Option<HirDefinitionId> {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn resolves_slots_shadowing_parameters_patterns_and_externals() {
        let program = parse(
            "hir.telora",
            "decl loop: Fn(Int) -> Int;\
             def loop = fn(n) { if n < 1 { n } else { loop(n - 1) } };\
             let f = fn(x) { let x = x; match ('Ok, x) { ('Ok, y) => y, _ => ext } };\
             f(loop(2))",
        )
        .unwrap();
        let hir = HirProgram::resolve(&program, ["Func".into(), "Int".into(), "ext".into()]);
        let unresolved = hir
            .unresolved()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(unresolved.is_empty(), "{unresolved:?}");
        let loop_definition = hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "loop")
            .unwrap();
        assert_eq!(loop_definition.additional_locations.len(), 1);
        assert!(hir.references().iter().any(|reference| {
            reference.name == "loop"
                && reference.resolution == HirResolution::Definition(loop_definition.id)
        }));
        assert!(hir.references().iter().any(|reference| {
            reference.name == "ext" && reference.resolution == HirResolution::External
        }));
        assert!(hir.expressions().len() > hir.references().len());
        assert!(
            hir.expressions()
                .iter()
                .any(|expression| expression.parent.is_some())
        );
    }

    #[test]
    fn retains_type_argument_placeholders_without_references() {
        let source = "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair[Int, _](1, \"x\")";
        let program = parse("hir.telora", source).unwrap();
        let hir = HirProgram::resolve(
            &program,
            ["for".into(), "Func".into(), "Int".into(), "pair".into()],
        );
        let placeholder = hir
            .expressions()
            .iter()
            .find(|expression| {
                expression.location.range()
                    == (source.find('_').unwrap()..source.find('_').unwrap() + 1)
            })
            .expect("placeholder expression");
        assert!(placeholder.reference.is_none());
        assert!(placeholder.parent.is_some());
    }

    #[test]
    fn interpreter_hir_indexes_only_authored_operand() {
        let program = parse(
            "hir.telora",
            "def lift: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool = interpreter!(eq_i); lift",
        )
        .unwrap();
        let hir = HirProgram::resolve(
            &program,
            ["Func", "TypeOf", "Bool", "eq_i"]
                .into_iter()
                .map(str::to_owned),
        );
        let names = hir
            .references()
            .iter()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"eq_i"));
        assert!(!names.iter().any(|name| name.contains("telora_interpreter")));
        assert!(!names.contains(&"\0telora_pack_dyn"));
    }

    #[test]
    fn blame_hir_indexes_arguments_but_not_the_intrinsic_name() {
        let program = parse(
            "hir.telora",
            "let data = 1; let message = \"bad\"; blame!(message, data)",
        )
        .unwrap();
        let hir = HirProgram::resolve(&program, std::iter::empty());
        let names = hir
            .references()
            .iter()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"data"));
        assert!(names.contains(&"message"));
        assert!(!names.contains(&"blame"));
    }
}
