#[derive(Clone, Copy)]
enum SurfaceRole<'a> {
    Type,
    Constructor,
    Namespace(&'a ModuleInterface),
    Data,
    Unresolved,
}

struct TypeBoundary<'a> {
    hir: &'a HirProgram,
    interfaces: &'a BTreeMap<String, ModuleInterface>,
    parameters: Vec<String>,
    constructors: HashSet<crate::Location>,
    external_data: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypeBoundary<'a> {
    fn new(hir: &'a HirProgram, interfaces: &'a BTreeMap<String, ModuleInterface>) -> Self {
        Self {
            hir,
            interfaces,
            parameters: Vec::new(),
            constructors: HashSet::new(),
            external_data: HashSet::new(),
            diagnostics: Vec::new(),
        }
    }

    fn interface_role(interface: &'a ModuleInterface, name: &str) -> SurfaceRole<'a> {
        if let Some(namespace) = interface.namespaces.get(name) {
            SurfaceRole::Namespace(namespace)
        } else if interface.type_declarations.contains(name) {
            if matches!(
                interface.member_constructors.get(name),
                Some(ValueConstructor::Newtype)
            ) {
                SurfaceRole::Constructor
            } else {
                SurfaceRole::Type
            }
        } else {
            SurfaceRole::Data
        }
    }

    fn external_role(&self, name: &str) -> SurfaceRole<'a> {
        if self
            .parameters
            .iter()
            .rev()
            .any(|parameter| parameter == name)
        {
            return SurfaceRole::Type;
        }
        if let Some(interface) = self.interfaces.get(name) {
            return if let Some(binding) = &interface.value_binding {
                Self::interface_role(interface, binding)
            } else {
                SurfaceRole::Namespace(interface)
            };
        }
        if self.external_data.contains(name) {
            return SurfaceRole::Data;
        }
        if matches!(
            name,
            "Type"
                | "Dyn"
                | "Never"
                | "Unit"
                | "Int"
                | "Float"
                | "String"
                | "Bytes"
                | "Bool"
                | "PropertyTarget"
                | "Array"
                | "Dict"
                | "Tuple"
                | "Func"
                | "TypeOf"
                | "Unchecked"
                | "Option"
                | "Result"
                | "FoldControl"
                | "Property"
                | "\0telora_unit_type"
                | "\0telora_tuple_type"
                | "\0telora_function_type"
        ) {
            SurfaceRole::Type
        } else {
            SurfaceRole::Data
        }
    }

    fn role(&self, expression: &Expr) -> SurfaceRole<'a> {
        self.role_with_path(expression, &mut Vec::new())
    }

    fn role_with_path(
        &self,
        expression: &Expr,
        path: &mut Vec<crate::Location>,
    ) -> SurfaceRole<'a> {
        match &expression.value {
            ExprKind::TypeSyntax(_) => SurfaceRole::Type,
            ExprKind::Variable(name) => {
                match self
                    .hir
                    .reference_at(name.location, &name.value)
                    .map(|reference| reference.resolution)
                {
                    Some(HirResolution::Definition(id)) => {
                        let definition = self.hir.definition(id).expect("resolved definition");
                        if let Some(import) = &definition.member_import {
                            if path.contains(&definition.location) {
                                return SurfaceRole::Data;
                            }
                            path.push(definition.location);
                            let role = self.role_with_path(import, path);
                            path.pop();
                            return role;
                        }
                        match definition.kind {
                            HirDefinitionKind::Type | HirDefinitionKind::NativeType => {
                                if self.constructors.contains(&definition.location) {
                                    SurfaceRole::Constructor
                                } else {
                                    SurfaceRole::Type
                                }
                            }
                            HirDefinitionKind::Import
                                if !self.interfaces.contains_key(&name.value) =>
                            {
                                SurfaceRole::Unresolved
                            }
                            HirDefinitionKind::Import => self.external_role(&name.value),
                            _ => SurfaceRole::Data,
                        }
                    }
                    Some(HirResolution::Unresolved) => SurfaceRole::Unresolved,
                    _ => self.external_role(&name.value),
                }
            }
            ExprKind::Field { receiver, field } => match self.role_with_path(receiver, path) {
                SurfaceRole::Namespace(interface) => Self::interface_role(interface, &field.value),
                SurfaceRole::Unresolved => SurfaceRole::Unresolved,
                _ => SurfaceRole::Data,
            },
            ExprKind::Call { callee, .. } => match self.role_with_path(callee, path) {
                role @ (SurfaceRole::Type | SurfaceRole::Constructor) => role,
                _ => SurfaceRole::Data,
            },
            _ => SurfaceRole::Data,
        }
    }

    fn invalid_type(&mut self, expression: &Expr) {
        self.diagnostics.push(Diagnostic::error(
            "expected a static type declaration, constructor or family; metadata data cannot become a type",
            expression.location,
        ));
    }

    fn type_expression(&mut self, expression: &Expr) {
        match &expression.value {
            ExprKind::TypeSyntax(inner) => self.type_expression(inner),
            ExprKind::Variable(_) | ExprKind::Field { .. } => {
                if !matches!(
                    self.role(expression),
                    SurfaceRole::Type | SurfaceRole::Constructor | SurfaceRole::Unresolved
                ) {
                    self.invalid_type(expression);
                }
            }
            ExprKind::Call { callee, arguments } => {
                if !matches!(
                    self.role(callee),
                    SurfaceRole::Type | SurfaceRole::Constructor | SurfaceRole::Unresolved
                ) {
                    self.invalid_type(callee);
                    return;
                }
                if matches!(callee.value, ExprKind::Call { .. }) {
                    self.type_expression(callee);
                }
                for argument in arguments {
                    if let ExprKind::Array(items) = &argument.value {
                        for item in items {
                            self.type_expression(item);
                        }
                    } else {
                        self.type_expression(argument);
                    }
                }
            }
            _ => self.invalid_type(expression),
        }
    }

    fn bindings(&mut self, bindings: &[Binding]) {
        for binding in bindings {
            if binding.value.declared_initializer
                == Some(crate::ast::DeclaredInitializerKind::Newtype)
            {
                self.constructors.insert(binding.value.name.location);
            }
        }
        loop {
            let before = self.constructors.len();
            for binding in bindings {
                if binding.value.kind != BindingKind::Type
                    || binding.value.declared_initializer.is_some()
                {
                    continue;
                }
                let mut value = &binding.value.value;
                while let ExprKind::TypeSyntax(inner) = &value.value {
                    value = inner;
                }
                if matches!(self.role(value), SurfaceRole::Constructor) {
                    self.constructors.insert(binding.value.name.location);
                }
            }
            if self.constructors.len() == before {
                break;
            }
        }
        for binding in bindings {
            let boundary = self.parameters.len();
            self.parameters.extend(
                binding
                    .value
                    .type_parameters
                    .iter()
                    .map(|parameter| parameter.value.clone()),
            );
            for bounds in &binding.value.type_parameter_bounds {
                for bound in bounds {
                    self.type_expression(bound);
                }
            }
            if let Some(annotation) = &binding.value.annotation {
                self.type_expression(annotation);
            }
            if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
                if binding.value.declared_initializer.is_some() {
                    // Nominal syntax lowers to a trusted model constructor with context and member types.
                    if let ExprKind::Call { arguments, .. } = &binding.value.value.value
                        && let Some(fields) = arguments.last()
                        && let ExprKind::Dict(fields) = &fields.value
                    {
                        for field in fields {
                            if !matches!(field.value.value.value, ExprKind::Atom(_)) {
                                self.type_expression(&field.value.value);
                            }
                            for decorator in &field.value.decorators {
                                self.decorator(decorator);
                            }
                        }
                    }
                } else {
                    self.type_expression(&binding.value.value);
                }
            } else if !binding.value.is_member_import()
                && !matches!(
                    binding.value.kind,
                    BindingKind::Decl
                        | BindingKind::Native
                        | BindingKind::NativeType
                        | BindingKind::Import
                        | BindingKind::OpenImport
                        | BindingKind::Export
                )
            {
                self.data_expression(&binding.value.value);
            }
            for decorator in &binding.value.decorators {
                self.decorator(decorator);
            }
            self.parameters.truncate(boundary);
        }
    }

    fn decorator(&mut self, decorator: &crate::ast::Decorator) {
        for argument in &decorator.value.arguments {
            self.data_expression(argument);
        }
    }

    fn block(&mut self, block: &Block) {
        self.bindings(&block.value.bindings);
        self.data_expression(&block.value.result);
    }

    fn data_expression(&mut self, expression: &Expr) {
        match &expression.value {
            ExprKind::TypeMetadata(inner) => self.type_expression(inner),
            ExprKind::TypeSyntax(_) | ExprKind::Variable(_) | ExprKind::Field { .. }
                if matches!(self.role(expression), SurfaceRole::Type) =>
            {
                self.diagnostics.push(Diagnostic::error(
                    "a type cannot be used as data; use '.type' for metadata",
                    expression.location,
                ));
            }
            ExprKind::Array(items) | ExprKind::Tuple(items) => {
                for item in items {
                    self.data_expression(item);
                }
            }
            ExprKind::Dict(fields) => {
                for field in fields {
                    self.data_expression(&field.value.value);
                }
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::TypeAscription { value, target }
            | ExprKind::CheckedCast { value, target } => {
                self.data_expression(value);
                self.type_expression(target);
            }
            ExprKind::DynProject {
                namespace,
                target,
                value,
            } => {
                self.data_expression(namespace);
                self.type_expression(target);
                self.data_expression(value);
            }
            ExprKind::Call { callee, arguments } => {
                if matches!(self.role(callee), SurfaceRole::Type) {
                    self.diagnostics.push(Diagnostic::error(
                        "a type constructor application is not data; use '(Type(...)).type' for metadata",
                        expression.location));
                } else {
                    self.data_expression(callee);
                }
                for argument in arguments {
                    self.data_expression(argument);
                }
            }
            ExprKind::TypeApply { callee, arguments } => {
                if !matches!(
                    self.role(callee),
                    SurfaceRole::Type | SurfaceRole::Constructor
                ) {
                    self.data_expression(callee);
                }
                for argument in arguments {
                    if let TypeArgumentKind::Explicit(argument) = &argument.value {
                        self.type_expression(argument);
                    }
                }
            }
            ExprKind::Closure {
                parameters,
                result_annotation,
                body,
            } => {
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        self.type_expression(annotation);
                    }
                }
                if let Some(annotation) = result_annotation {
                    self.type_expression(annotation);
                }
                self.block(body);
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.data_expression(condition);
                self.block(then_branch);
                self.block(else_branch);
            }
            ExprKind::IfLet {
                value,
                then_branch,
                else_branch,
                ..
            } => {
                self.data_expression(value);
                self.block(then_branch);
                self.block(else_branch);
            }
            ExprKind::LetElse {
                value,
                else_branch,
                body,
                ..
            } => {
                self.data_expression(value);
                self.block(else_branch);
                self.block(body);
            }
            ExprKind::Match { value, arms } => {
                self.data_expression(value);
                for arm in arms {
                    if let Some(guard) = &arm.value.guard {
                        self.data_expression(guard);
                    }
                    self.data_expression(&arm.value.value);
                }
            }
            ExprKind::Field { receiver, .. } => {
                if !matches!(
                    self.role(receiver),
                    SurfaceRole::Type | SurfaceRole::Constructor | SurfaceRole::Namespace(_)
                ) {
                    self.data_expression(receiver);
                }
            }
            ExprKind::Spread(value)
            | ExprKind::Unary { operand: value, .. }
            | ExprKind::Propagate { operand: value }
            | ExprKind::Return { value }
            | ExprKind::Panic { message: value }
            | ExprKind::Debug { value, .. }
            | ExprKind::TupleProjection {
                receiver: value, ..
            }
            | ExprKind::FieldProjection {
                receiver: value, ..
            } => self.data_expression(value),
            ExprKind::Binary { left, right, .. }
            | ExprKind::Index {
                receiver: left,
                index: right,
            }
            | ExprKind::Interpreter {
                operand: left,
                elaboration: right,
            } => {
                self.data_expression(left);
                self.data_expression(right);
            }
            ExprKind::Raise {
                message, subjects, ..
            } => {
                self.data_expression(message);
                for subject in subjects {
                    self.data_expression(subject);
                }
            }
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(value) = &part.value {
                        self.data_expression(value);
                    }
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bytes(_)
            | ExprKind::Atom(_)
            | ExprKind::Variable(_)
            | ExprKind::TypeSyntax(_) => {}
        }
    }
}
