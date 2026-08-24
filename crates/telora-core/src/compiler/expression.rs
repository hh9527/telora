impl<'a> Compiler<'a> {
    fn compile_expr(&mut self, expression: &Expr) -> Result<RegisterId, FrontendError> {
        let payload = self.compile_expr_unowned(expression)?;
        let Some(owner) = self
            .declared_value_owners
            .get(&expression.location)
            .cloned()
        else {
            return Ok(payload);
        };
        let owner = self
            .environment
            .get(&owner)
            .copied()
            .unwrap_or_else(|| self.load_external_constant(owner, expression.location));
        let result = self.allocate();
        self.emit(
            Operation::OwnDeclared {
                dst: result,
                owner,
                value: payload,
            },
            expression.location,
        );
        Ok(result)
    }

    fn compile_expr_unowned(&mut self, expression: &Expr) -> Result<RegisterId, FrontendError> {
        match &expression.value {
            ExprKind::Int(value) => {
                Ok(self.load_constant(Constant::Int(*value), expression.location))
            }
            ExprKind::Float(value) => {
                Ok(self.load_constant(Constant::Float(*value), expression.location))
            }
            ExprKind::String(value) => {
                Ok(self.load_constant(Constant::String(value.clone().into()), expression.location))
            }
            ExprKind::InterpolatedString(parts) => {
                let mut registers = Vec::with_capacity(parts.len());
                for part in parts {
                    registers.push(match &part.value {
                        StringPartKind::Text(text) => {
                            self.load_constant(Constant::String(text.clone().into()), part.location)
                        }
                        StringPartKind::Expression(expression) => self.compile_expr(expression)?,
                    });
                }
                let dst = self.allocate();
                self.emit(
                    Operation::InterpolateString {
                        dst,
                        parts: registers,
                    },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::Bytes(value) => {
                Ok(self.load_constant(Constant::Bytes(value.clone().into()), expression.location))
            }
            ExprKind::Atom(name) => {
                Ok(self.load_constant(atom_constant(name), expression.location))
            }
            ExprKind::Variable(name) => {
                let register = self.environment.get(&name.value).copied().ok_or_else(|| {
                    self.error_at(
                        expression.location,
                        format!("unknown binding {:?}", name.value),
                    )
                })?;
                if self.type_slot_bindings.contains(&name.value)
                    && !self.preserved_type_slot_reads.contains(&name.value)
                {
                    let dst = self.allocate();
                    self.emit(
                        Operation::ReadTypeSlot {
                            dst,
                            link: register,
                        },
                        expression.location,
                    );
                    Ok(dst)
                } else {
                    Ok(register)
                }
            }
            ExprKind::Array(items) => {
                if items
                    .iter()
                    .any(|item| matches!(item.value, ExprKind::Spread(_)))
                {
                    return self.compile_spread_array(items, expression.location);
                }
                let items = self.compile_many(items)?;
                let dst = self.allocate();
                self.emit(Operation::MakeArray { dst, items }, expression.location);
                Ok(dst)
            }
            ExprKind::Spread(_) => {
                Err(self.error_at(expression.location, "spread is only valid in a collection"))
            }
            ExprKind::Tuple(items) => {
                let items = self.compile_many(items)?;
                let dst = self.allocate();
                self.emit(Operation::MakeTuple { dst, items }, expression.location);
                Ok(dst)
            }
            ExprKind::Dict(fields) => self.compile_dict(fields, expression.location),
            ExprKind::Block(block) => self.compile_block(block),
            ExprKind::Unary { operator, operand } => {
                let src = self.compile_expr(operand)?;
                let dst = self.allocate();
                match operator.value {
                    UnaryOperator::Negate => {
                        self.emit(Operation::Negate { dst, src }, expression.location);
                    }
                    UnaryOperator::Not => {
                        self.emit(Operation::Not { dst, src }, expression.location);
                    }
                    UnaryOperator::LogicalNot => {
                        self.emit(Operation::LogicalNot { dst, src }, expression.location);
                    }
                    UnaryOperator::BitNot => {
                        self.emit(Operation::BitNot { dst, src }, expression.location);
                    }
                }
                Ok(dst)
            }
            ExprKind::Propagate { .. } => {
                Err(self.error_at(expression.location, "unelaborated propagation expression"))
            }
            ExprKind::Return { value } => {
                let value = self.compile_expr(value)?;
                self.emit(Operation::Return { src: value }, expression.location);
                Ok(value)
            }
            ExprKind::Panic { message } => {
                let message = self.compile_expr(message)?;
                self.emit(Operation::Panic { message }, expression.location);
                Ok(message)
            }
            ExprKind::Raise { error } => {
                let error = self.compile_expr(error)?;
                self.emit(Operation::Raise { error }, expression.location);
                Ok(error)
            }
            ExprKind::Debug {
                value,
                message,
                expression: source_expression,
            } => {
                let value = self.compile_expr(value)?;
                let source_file = self
                    .source_file
                    .expect("debug expressions require a registered source");
                let position = source_file.position(expression.location.start);
                self.emit(
                    Operation::Debug {
                        value,
                        module: self.source_name.to_owned(),
                        line: u32::try_from(position.line).unwrap_or(u32::MAX),
                        name: source_expression.clone(),
                        message: message.clone(),
                    },
                    expression.location,
                );
                Ok(value)
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.compile_expr(left)?;
                let right = self.compile_expr(right)?;
                let dst = self.allocate();
                let operation = match operator.value {
                    BinaryOperator::Add => Operation::Add { dst, left, right },
                    BinaryOperator::Subtract => Operation::Subtract { dst, left, right },
                    BinaryOperator::Multiply => Operation::Multiply { dst, left, right },
                    BinaryOperator::Divide => Operation::Divide { dst, left, right },
                    BinaryOperator::Remainder => Operation::Remainder { dst, left, right },
                    BinaryOperator::LessThan => Operation::LessThan { dst, left, right },
                    BinaryOperator::LessThanOrEqual => {
                        Operation::LessThanOrEqual { dst, left, right }
                    }
                    BinaryOperator::GreaterThan => Operation::LessThan {
                        dst,
                        left: right,
                        right: left,
                    },
                    BinaryOperator::GreaterThanOrEqual => Operation::LessThanOrEqual {
                        dst,
                        left: right,
                        right: left,
                    },
                    BinaryOperator::Equal => Operation::Equal { dst, left, right },
                    BinaryOperator::NotEqual => Operation::NotEqual { dst, left, right },
                    BinaryOperator::BitAnd => Operation::BitAnd { dst, left, right },
                    BinaryOperator::BitOr => Operation::BitOr { dst, left, right },
                    BinaryOperator::BitXor => Operation::BitXor { dst, left, right },
                    BinaryOperator::And | BinaryOperator::Or => {
                        return Err(self.error_at(
                            expression.location,
                            "unelaborated short-circuit expression",
                        ));
                    }
                };
                self.emit(operation, expression.location);
                Ok(dst)
            }
            ExprKind::TypeAscription { value, .. } => self.compile_expr(value),
            ExprKind::CheckedCast { value, target } => {
                let hidden = located(
                    ExprKind::Variable(located("\0telora_cast".to_owned(), expression.location)),
                    expression.location,
                );
                let call = located(
                    ExprKind::Call {
                        callee: Box::new(hidden),
                        arguments: vec![(**target).clone(), (**value).clone()],
                    },
                    expression.location,
                );
                self.compile_expr_unowned(&call)
            }
            ExprKind::DynProject {
                namespace,
                target,
                value,
            } => {
                let callee = located(
                    ExprKind::Field {
                        receiver: namespace.clone(),
                        field: located("project_with".to_owned(), expression.location),
                    },
                    expression.location,
                );
                let call = located(
                    ExprKind::Call {
                        callee: Box::new(callee),
                        arguments: vec![(**target).clone(), (**value).clone()],
                    },
                    expression.location,
                );
                self.compile_expr_unowned(&call)
            }
            ExprKind::Field { receiver, field } => {
                let dict = self.compile_expr(receiver)?;
                let dst = self.allocate();
                self.emit(
                    Operation::GetField {
                        dst,
                        dict,
                        field: field.value.clone(),
                    },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::Index { receiver, index } => {
                let array = self.compile_expr(receiver)?;
                let index = self.compile_expr(index)?;
                let dst = self.allocate();
                self.emit(
                    Operation::GetArray { dst, array, index },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::TupleProjection { receiver, index } => {
                let tuple = self.compile_expr(receiver)?;
                let dst = self.allocate();
                self.emit(
                    Operation::ProjectTuple {
                        dst,
                        tuple,
                        index: index.value,
                    },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::Call { callee, arguments } => {
                let (base, argument_count) =
                    self.compile_call_window(callee, arguments, expression.location)?;
                self.emit(
                    Operation::Call {
                        base,
                        argument_count,
                    },
                    expression.location,
                );
                Ok(base)
            }
            ExprKind::TypeApply { callee, .. } => self.compile_expr(callee),
            ExprKind::Interpreter { elaboration, .. } => self.compile_expr(elaboration),
            ExprKind::Closure {
                parameters, body, ..
            } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>();
                self.compile_closure(&parameters, body, expression.location)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.compile_if(condition, then_branch, else_branch, expression.location),
            ExprKind::IfLet { .. } => {
                Err(self.error_at(expression.location, "unelaborated if let expression"))
            }
            ExprKind::LetElse { .. } => {
                Err(self.error_at(expression.location, "unelaborated let else expression"))
            }
            ExprKind::Match { value, arms } => self.compile_match(value, arms, expression.location),
        }
    }

    fn compile_tail_expr(&mut self, expression: &Expr) -> Result<(), FrontendError> {
        if self
            .declared_value_owners
            .contains_key(&expression.location)
        {
            let result = self.compile_expr(expression)?;
            self.emit_synthetic(Operation::Return { src: result }, expression.location);
            return Ok(());
        }
        match &expression.value {
            ExprKind::Call { callee, arguments } => {
                let (base, argument_count) =
                    self.compile_call_window(callee, arguments, expression.location)?;
                self.emit(
                    Operation::TailCall {
                        base,
                        argument_count,
                    },
                    expression.location,
                );
            }
            ExprKind::Block(block) => self.compile_tail_block(block)?,
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.compile_tail_if(condition, then_branch, else_branch)?,
            ExprKind::Match { value, arms } => {
                self.compile_tail_match(value, arms, expression.location)?
            }
            ExprKind::Return { .. } => {
                self.compile_expr(expression)?;
            }
            _ => {
                let result = self.compile_expr(expression)?;
                self.emit_synthetic(Operation::Return { src: result }, expression.location);
            }
        }
        Ok(())
    }

    fn compile_call_window(
        &mut self,
        callee: &Expr,
        arguments: &[Expr],
        location: Location,
    ) -> Result<(RegisterId, u32), FrontendError> {
        let callee = self.compile_expr(callee)?;
        let arguments = self.compile_many(arguments)?;
        let base = self.allocate();
        self.emit(
            Operation::Move {
                dst: base,
                src: callee,
            },
            location,
        );
        for argument in &arguments {
            let destination = self.allocate();
            self.emit(
                Operation::Move {
                    dst: destination,
                    src: *argument,
                },
                location,
            );
        }
        let argument_count = u32::try_from(arguments.len())
            .map_err(|_| frontend_error(self.source_name, "too many call arguments"))?;
        Ok((base, argument_count))
    }

    fn compile_many(&mut self, expressions: &[Expr]) -> Result<Vec<RegisterId>, FrontendError> {
        expressions
            .iter()
            .map(|expression| self.compile_expr(expression))
            .collect()
    }

    fn compile_spread_array(
        &mut self,
        items: &[Expr],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let mut arrays = Vec::with_capacity(items.len());
        for item in items {
            if let ExprKind::Spread(operand) = &item.value {
                arrays.push(self.compile_expr(operand)?);
            } else {
                let value = self.compile_expr(item)?;
                let array = self.allocate();
                self.emit(
                    Operation::MakeArray {
                        dst: array,
                        items: vec![value],
                    },
                    item.location,
                );
                arrays.push(array);
            }
        }
        let dst = self.allocate();
        self.emit(Operation::ConcatArrays { dst, arrays }, location);
        Ok(dst)
    }

    fn compile_dict(
        &mut self,
        fields: &[DictField],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        if fields.iter().any(|field| field.value.name.is_none()) {
            return self.compile_spread_dict(fields, location);
        }
        let mut seen = HashSet::new();
        let mut compiled = Vec::with_capacity(fields.len());
        for field in fields {
            let name = &field
                .value
                .name
                .as_ref()
                .expect("ordinary Dict field has a name")
                .value;
            if !seen.insert(name) {
                return Err(frontend_error(
                    self.source_name,
                    format!("duplicate Dict field {name:?}"),
                ));
            }
            compiled.push((name.clone(), self.compile_expr(&field.value.value)?));
        }
        let dst = self.allocate();
        self.emit(
            Operation::MakeDict {
                dst,
                fields: compiled,
            },
            location,
        );
        Ok(dst)
    }

    fn compile_spread_dict(
        &mut self,
        fields: &[DictField],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let mut seen = HashSet::new();
        let mut dicts = Vec::with_capacity(fields.len());
        for field in fields {
            if let Some(name) = &field.value.name {
                if !seen.insert(&name.value) {
                    return Err(frontend_error(
                        self.source_name,
                        format!("duplicate Dict field {:?}", name.value),
                    ));
                }
                let value = self.compile_expr(&field.value.value)?;
                let dict = self.allocate();
                self.emit(
                    Operation::MakeDict {
                        dst: dict,
                        fields: vec![(name.value.clone(), value)],
                    },
                    field.location,
                );
                dicts.push(dict);
            } else {
                let ExprKind::Spread(operand) = &field.value.value.value else {
                    return Err(self.error_at(field.location, "invalid Dict spread entry"));
                };
                dicts.push(self.compile_expr(operand)?);
            }
        }
        let dst = self.allocate();
        self.emit(Operation::MergeDicts { dst, dicts }, location);
        Ok(dst)
    }

}
