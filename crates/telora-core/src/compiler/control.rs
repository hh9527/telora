impl<'a> Compiler<'a> {
    fn compile_closure(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        self.compile_closure_with_declared_family(parameters, body, location, None)
    }

    fn compile_memoized_interpreter_closure(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        self.compile_closure_with_mode(parameters, body, location, None, true)
    }

    fn compile_closure_with_declared_family(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        location: Location,
        nominal_constructor: Option<NominalTypeConstructor>,
    ) -> Result<RegisterId, FrontendError> {
        self.compile_closure_with_mode(parameters, body, location, nominal_constructor, false)
    }

    fn compile_closure_with_mode(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        location: Location,
        nominal_constructor: Option<NominalTypeConstructor>,
        memoized_interpreter: bool,
    ) -> Result<RegisterId, FrontendError> {
        let mut bound = parameters
            .iter()
            .map(|parameter| parameter.value.clone())
            .collect::<HashSet<_>>();
        if bound.len() != parameters.len() {
            return Err(frontend_error(
                self.source_name,
                "duplicate closure parameter",
            ));
        }
        let mut free = BTreeSet::new();
        free_block(body, &mut bound, &mut free);
        let mut captures = free.into_iter().collect::<Vec<_>>();
        let mut capture_registers = Vec::with_capacity(captures.len());
        for name in &captures {
            let register = if let Some(register) = self.environment.get(name).copied() {
                register
            } else {
                return Err(frontend_error(
                    self.source_name,
                    format!("unknown binding {name:?}"),
                ));
            };
            if self.ready_type_slot_bindings.contains(name) {
                let value = self.allocate();
                self.emit(
                    Operation::ReadTypeSlot {
                        dst: value,
                        link: register,
                    },
                    location,
                );
                capture_registers.push(value);
            } else {
                capture_registers.push(register);
            }
        }
        for owner in self
            .declared_value_owners
            .iter()
            .filter(|(owner_location, _)| {
                body.location.start <= owner_location.start
                    && owner_location.end <= body.location.end
            })
            .map(|(_, owner)| owner)
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            if captures.contains(&owner) {
                continue;
            }
            let register = self
                .environment
                .get(&owner)
                .copied()
                .unwrap_or_else(|| self.load_external_constant(owner.clone(), location));
            captures.push(owner);
            capture_registers.push(register);
        }
        let captured_type_slots = captures
            .iter()
            .filter(|name| {
                self.type_slot_bindings.contains(*name)
                    && !self.ready_type_slot_bindings.contains(*name)
            })
            .cloned()
            .collect::<HashSet<_>>();
        let captured_definitions = captures
            .iter()
            .filter(|name| self.definition_bindings.contains(*name))
            .cloned()
            .collect::<HashSet<_>>();

        let name = format!("{}::closure{}", self.function_name, self.closure_index);
        self.closure_index += 1;
        let mut nested = Self::nested(
            self.source_name,
            self.source_file,
            name,
            parameters,
            NestedEnvironment {
                captures: &captures,
                type_slots: &captured_type_slots,
                definitions: &captured_definitions,
                declared_value_owners: &self.declared_value_owners,
            },
        )?;
        if let Some(constructor) = nominal_constructor {
            let structural = nested.compile_block(body)?;
            let native = Constant::Native(NativeFunction::new(
                "type-family.declare",
                parameters.len() + 4,
                crate::types::native_declare_type_family,
            ));
            let native = nested.load_constant(native, location);
            let module = nested.load_constant(
                Constant::Int(i64::from(constructor.id.module.raw())),
                location,
            );
            let local =
                nested.load_constant(Constant::Int(i64::from(constructor.id.local)), location);
            let name = nested.load_constant(Constant::String(constructor.name.into()), location);
            let base = nested.allocate();
            nested.emit(
                Operation::Move {
                    dst: base,
                    src: native,
                },
                location,
            );
            for source in std::iter::once(structural)
                .chain(std::iter::once(module))
                .chain(std::iter::once(local))
                .chain(std::iter::once(name))
                .chain((0..parameters.len()).map(|index| RegisterId(index as u32)))
            {
                let destination = nested.allocate();
                nested.emit(
                    Operation::Move {
                        dst: destination,
                        src: source,
                    },
                    location,
                );
            }
            nested.emit(
                Operation::Call {
                    base,
                    argument_count: u32::try_from(parameters.len() + 4).map_err(|_| {
                        frontend_error(self.source_name, "too many type parameters")
                    })?,
                },
                location,
            );
            nested.emit(Operation::Return { src: base }, location);
        } else {
            nested.compile_tail_block(body)?;
        }
        let mut function = nested.finish_lir();
        function.memoized_interpreter = memoized_interpreter;
        let function = Box::new(function);

        let dst = self.allocate();
        self.emit(
            Operation::MakeClosure {
                dst,
                function,
                captures: capture_registers,
            },
            location,
        );
        Ok(dst)
    }

    fn compile_if(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: &Block,
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let condition_location = condition.location;
        let condition = self.compile_expr(condition)?;
        let else_label = self.new_label();
        self.emit(
            Operation::JumpIfFalse {
                condition,
                target: else_label,
            },
            condition_location,
        );
        let then_value = self.compile_block(then_branch)?;
        let result = self.allocate();
        self.emit_synthetic(
            Operation::Move {
                dst: result,
                src: then_value,
            },
            then_branch.location,
        );
        let end_label = self.new_label();
        self.emit_synthetic(Operation::Jump { target: end_label }, location);
        self.mark_label(else_label);
        let else_value = self.compile_block(else_branch)?;
        self.emit_synthetic(
            Operation::Move {
                dst: result,
                src: else_value,
            },
            else_branch.location,
        );
        self.mark_label(end_label);
        Ok(result)
    }

    fn compile_tail_if(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: &Block,
    ) -> Result<(), FrontendError> {
        let condition_location = condition.location;
        let condition = self.compile_expr(condition)?;
        let else_label = self.new_label();
        self.emit(
            Operation::JumpIfFalse {
                condition,
                target: else_label,
            },
            condition_location,
        );
        self.compile_tail_block(then_branch)?;
        self.mark_label(else_label);
        self.compile_tail_block(else_branch)
    }

    fn compile_match(
        &mut self,
        value: &Expr,
        arms: &[MatchArm],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let value = self.compile_expr(value)?;
        let result = self.allocate();
        let mut end_jumps = Vec::new();

        for arm in arms {
            let outer = self.environment.clone();
            let mut failures = Vec::new();
            let mut pattern_bindings = HashSet::new();
            self.compile_pattern(
                &arm.value.pattern,
                value,
                &mut failures,
                &mut pattern_bindings,
            )?;
            if let Some(guard) = &arm.value.guard {
                let condition = self.compile_expr(guard)?;
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    guard.location,
                );
                failures.push(failure);
            }
            let arm_value = self.compile_expr(&arm.value.value)?;
            self.emit_synthetic(
                Operation::Move {
                    dst: result,
                    src: arm_value,
                },
                arm.location,
            );
            let end = self.new_label();
            self.emit_synthetic(Operation::Jump { target: end }, arm.location);
            end_jumps.push(end);
            for failure in failures {
                self.mark_label(failure);
            }
            self.environment = outer;
        }

        self.emit(
            Operation::Fail {
                message: "no match arm accepted the value".into(),
            },
            location,
        );
        for jump in end_jumps {
            self.mark_label(jump);
        }
        Ok(result)
    }

    fn compile_tail_match(
        &mut self,
        value: &Expr,
        arms: &[MatchArm],
        location: Location,
    ) -> Result<(), FrontendError> {
        let value = self.compile_expr(value)?;

        for arm in arms {
            let outer = self.environment.clone();
            let mut failures = Vec::new();
            let mut pattern_bindings = HashSet::new();
            self.compile_pattern(
                &arm.value.pattern,
                value,
                &mut failures,
                &mut pattern_bindings,
            )?;
            if let Some(guard) = &arm.value.guard {
                let condition = self.compile_expr(guard)?;
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    guard.location,
                );
                failures.push(failure);
            }
            self.compile_tail_expr(&arm.value.value)?;
            for failure in failures {
                self.mark_label(failure);
            }
            self.environment = outer;
        }

        self.emit(
            Operation::Fail {
                message: "no match arm accepted the value".into(),
            },
            location,
        );
        Ok(())
    }

    fn compile_pattern(
        &mut self,
        pattern: &Pattern,
        value: RegisterId,
        failures: &mut Vec<LabelId>,
        bindings: &mut HashSet<String>,
    ) -> Result<(), FrontendError> {
        match &pattern.value {
            PatternKind::Wildcard => {}
            PatternKind::Binding(name) => {
                if !bindings.insert(name.value.clone()) {
                    return Err(frontend_error(
                        self.source_name,
                        format!("duplicate pattern binding {:?}", name.value),
                    ));
                }
                self.environment.insert(name.value.clone(), value);
            }
            PatternKind::Int(item) => {
                let expected = self.load_constant(Constant::Int(*item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Float(item) => {
                let expected = self.load_constant(Constant::Float(*item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::String(item) => {
                let expected =
                    self.load_constant(Constant::String(item.clone().into()), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Atom(item) => {
                let expected = self.load_constant(atom_constant(item), pattern.location);
                let condition = self.allocate();
                self.emit(
                    Operation::TaggedTagEquals {
                        dst: condition,
                        value,
                        tag: expected,
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
            }
            PatternKind::Tagged { tag, payload } => {
                let expected = self.load_constant(atom_constant(tag), pattern.location);
                let condition = self.allocate();
                self.emit(
                    Operation::TaggedTagEquals {
                        dst: condition,
                        value,
                        tag: expected,
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
                let payload_value = self.allocate();
                self.emit(
                    Operation::GetTaggedPayload {
                        dst: payload_value,
                        value,
                    },
                    pattern.location,
                );
                self.compile_pattern(payload, payload_value, failures, bindings)?;
            }
            PatternKind::Tuple(items) => {
                let condition = self.allocate();
                self.emit(
                    Operation::TupleLengthEquals {
                        dst: condition,
                        value,
                        length: items.len(),
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
                for (index, pattern) in items.iter().enumerate() {
                    let element = self.allocate();
                    self.emit(
                        Operation::GetTuple {
                            dst: element,
                            tuple: value,
                            index,
                        },
                        pattern.location,
                    );
                    self.compile_pattern(pattern, element, failures, bindings)?;
                }
            }
            PatternKind::Struct(fields) => {
                let condition = self.allocate();
                self.emit(
                    Operation::IsDict {
                        dst: condition,
                        value,
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
                let mut field_names = HashSet::new();
                for field in fields {
                    if !field_names.insert(field.name.value.clone()) {
                        return Err(frontend_error(
                            self.source_name,
                            format!("duplicate Struct pattern field {:?}", field.name.value),
                        ));
                    }
                    let condition = self.allocate();
                    self.emit(
                        Operation::FieldExists {
                            dst: condition,
                            value,
                            field: field.name.value.clone(),
                        },
                        field.name.location,
                    );
                    let failure = self.new_label();
                    self.emit(
                        Operation::JumpIfFalse {
                            condition,
                            target: failure,
                        },
                        field.name.location,
                    );
                    failures.push(failure);
                    let selected = self.allocate();
                    self.emit(
                        Operation::GetField {
                            dst: selected,
                            dict: value,
                            field: field.name.value.clone(),
                        },
                        field.name.location,
                    );
                    self.compile_pattern(&field.pattern, selected, failures, bindings)?;
                }
            }
        }
        Ok(())
    }

    fn emit_pattern_equality(
        &mut self,
        value: RegisterId,
        expected: RegisterId,
        failures: &mut Vec<LabelId>,
        location: Location,
    ) {
        let condition = self.allocate();
        self.emit(
            Operation::Equal {
                dst: condition,
                left: value,
                right: expected,
            },
            location,
        );
        let failure = self.new_label();
        self.emit(
            Operation::JumpIfFalse {
                condition,
                target: failure,
            },
            location,
        );
        failures.push(failure);
    }

    fn load_constant(&mut self, value: Constant, location: Location) -> RegisterId {
        let constant = self.constants.len();
        self.constants.push(value);
        let dst = self.allocate();
        self.emit(
            Operation::LoadConst {
                dst,
                constant: ConstantId(u32::try_from(constant).expect("constant pool exceeds u32")),
            },
            location,
        );
        dst
    }

    fn load_external_constant(&mut self, key: String, location: Location) -> RegisterId {
        let index = self.constants.len();
        let register = self.load_constant(Constant::Placeholder, location);
        self.external_constant_links.push((index, key));
        register
    }

    fn allocate(&mut self) -> RegisterId {
        let register = RegisterId(self.next_register);
        self.next_register = self
            .next_register
            .checked_add(1)
            .expect("register count exceeds u32");
        register
    }

    fn emit(&mut self, operation: Operation, location: Location) {
        self.items.push(Item::Operation(WithOrigin {
            value: operation,
            origin: Origin::Source(location),
        }));
    }

    fn emit_synthetic(&mut self, operation: Operation, derived_from: Location) {
        self.items.push(Item::Operation(WithOrigin {
            value: operation,
            origin: Origin::Synthetic {
                derived_from: Some(derived_from),
            },
        }));
    }

    fn new_label(&mut self) -> LabelId {
        let label = LabelId(self.next_label);
        self.next_label = self
            .next_label
            .checked_add(1)
            .expect("label count exceeds u32");
        label
    }

    fn mark_label(&mut self, label: LabelId) {
        self.items.push(Item::Label(label));
    }
}
