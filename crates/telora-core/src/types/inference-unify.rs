impl<'a> GenericInference<'a> {
    fn require_numeric(&mut self, ty: &TypeDescriptor) -> Result<(), String> {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.numeric_variables.insert(variable);
                Ok(())
            }
            TypeDescriptor::Int
            | TypeDescriptor::Float
            | TypeDescriptor::Any
            | TypeDescriptor::Never => Ok(()),
            ty => Err(format!(
                "numeric operator requires Int or Float, found {}",
                ty.display_name()
            )),
        }
    }

    fn require_not_operand(&mut self, ty: &TypeDescriptor) -> Result<(), String> {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.not_variables.insert(variable);
                Ok(())
            }
            TypeDescriptor::Int | TypeDescriptor::Any | TypeDescriptor::Never => Ok(()),
            TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::True | BuiltinAtom::False)) => Ok(()),
            TypeDescriptor::Enum(variants)
                if TypeDescriptor::Enum(variants.clone()) == normalized_bool_descriptor() =>
            {
                Ok(())
            }
            ty => Err(format!(
                "! requires Int or Bool, found {}",
                ty.display_name()
            )),
        }
    }

    fn require_ordered(&mut self, ty: &TypeDescriptor) -> Result<(), String> {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.ordered_variables.insert(variable);
                Ok(())
            }
            TypeDescriptor::Int
            | TypeDescriptor::Float
            | TypeDescriptor::String
            | TypeDescriptor::Any
            | TypeDescriptor::Never => Ok(()),
            ty => Err(format!(
                "ordered comparison requires Int, Float, or String, found {}",
                ty.display_name()
            )),
        }
    }

    fn bind_inference_variable(
        &mut self,
        variable: InferenceVariableId,
        ty: &TypeDescriptor,
    ) -> Result<(), String> {
        let ty = self.resolve(ty);
        if self.occurs(variable, &ty) {
            return Err(format!("infinite type for ?{}", variable.0));
        }
        if self.numeric_variables.contains(&variable) {
            match &ty {
                TypeDescriptor::Inference(target) => {
                    self.numeric_variables.insert(*target);
                }
                TypeDescriptor::Int
                | TypeDescriptor::Float
                | TypeDescriptor::Any
                | TypeDescriptor::Never => {}
                _ => {
                    return Err(format!(
                        "numeric operator requires Int or Float, found {}",
                        ty.display_name()
                    ));
                }
            }
        }
        if let TypeDescriptor::Inference(target) = ty
            && self.numeric_variables.contains(&target)
        {
            self.numeric_variables.insert(variable);
        }
        if self.not_variables.contains(&variable) {
            match &ty {
                TypeDescriptor::Inference(target) => {
                    self.not_variables.insert(*target);
                }
                TypeDescriptor::Int | TypeDescriptor::Any | TypeDescriptor::Never => {}
                TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::True | BuiltinAtom::False)) => {}
                TypeDescriptor::Enum(variants)
                    if TypeDescriptor::Enum(variants.clone()) == normalized_bool_descriptor() => {}
                _ => {
                    return Err(format!(
                        "! requires Int or Bool, found {}",
                        ty.display_name()
                    ));
                }
            }
        }
        if let TypeDescriptor::Inference(target) = ty
            && self.not_variables.contains(&target)
        {
            self.not_variables.insert(variable);
        }
        if self.ordered_variables.contains(&variable) {
            match &ty {
                TypeDescriptor::Inference(target) => {
                    self.ordered_variables.insert(*target);
                }
                TypeDescriptor::Int
                | TypeDescriptor::Float
                | TypeDescriptor::String
                | TypeDescriptor::Any
                | TypeDescriptor::Never => {}
                _ => {
                    return Err(format!(
                        "ordered comparison requires Int, Float, or String, found {}",
                        ty.display_name()
                    ));
                }
            }
        }
        if let TypeDescriptor::Inference(target) = ty
            && self.ordered_variables.contains(&target)
        {
            self.ordered_variables.insert(variable);
        }
        if let Some(requirements) = self.field_requirements.remove(&variable) {
            if let TypeDescriptor::Inference(target) = &ty {
                let mut merged = self.field_requirements.remove(target).unwrap_or_default();
                for (field, result) in requirements {
                    if let Some(existing) = merged.get(&field).cloned() {
                        self.unify(&result, &existing)?;
                    } else {
                        merged.insert(field, result);
                    }
                }
                self.field_requirements.insert(*target, merged);
            } else {
                for (field, result) in requirements {
                    let projected = self.project_field(&ty, &field)?;
                    self.check(&projected, &result)?;
                }
            }
        }
        self.substitutions.insert(variable, ty);
        Ok(())
    }

    fn unify(&mut self, left: &TypeDescriptor, right: &TypeDescriptor) -> Result<(), String> {
        if let Some(query) = &self.query {
            query.check().map_err(|error| error.to_string())?;
        }
        let completed_left = self.complete_declared(left);
        let completed_right = self.complete_declared(right);
        let left = completed_left.as_ref().unwrap_or(left);
        let right = completed_right.as_ref().unwrap_or(right);
        if matches!(left, TypeDescriptor::Never) {
            return Ok(());
        }
        if let TypeDescriptor::Inference(variable) = left
            && let Some(existing) = self.substitutions.get(variable).cloned()
            && self.declared_identity(right).is_some()
        {
            if matches!(existing, TypeDescriptor::Inference(_)) {
                return self.unify(&existing, right);
            }
            let compatibility = match right {
                TypeDescriptor::Declared(declared) => declared.body.as_ref(),
                _ => right,
            };
            self.check(&existing, compatibility)?;
            self.substitutions.insert(*variable, right.clone());
            return Ok(());
        }
        if let TypeDescriptor::Inference(variable) = right
            && let Some(existing) = self.substitutions.get(variable).cloned()
            && self.declared_identity(left).is_some()
        {
            if matches!(existing, TypeDescriptor::Inference(_)) {
                return self.unify(left, &existing);
            }
            let compatibility = match left {
                TypeDescriptor::Declared(declared) => declared.body.as_ref(),
                _ => left,
            };
            self.check(&existing, compatibility)?;
            self.substitutions.insert(*variable, left.clone());
            return Ok(());
        }
        if let (Some(left), Some(right)) =
            (self.declared_identity(left), self.declared_identity(right))
            && left.has_same_head(&right)
            && left.arguments().len() == right.arguments().len()
        {
            for (left, right) in left.arguments().iter().zip(right.arguments()) {
                self.unify(left, right)?;
            }
            return Ok(());
        }
        if let (TypeDescriptor::TypeOf(left), TypeDescriptor::TypeOf(right)) = (left, right) {
            return self.unify(left, right);
        }
        let left = self.resolve(left);
        let right = self.resolve(right);
        match (&left, &right) {
            (TypeDescriptor::Declared(declared), other)
                if !matches!(other, TypeDescriptor::Declared(_))
                    && declared.body.as_ref() == other =>
            {
                return Ok(());
            }
            (other, TypeDescriptor::Declared(declared))
                if !matches!(other, TypeDescriptor::Declared(_))
                    && other == declared.body.as_ref() =>
            {
                return Ok(());
            }
            _ => {}
        }
        if let (TypeDescriptor::Struct(fields), TypeDescriptor::Dict(item)) = (&left, &right) {
            for field in fields.values() {
                self.unify(field, item)?;
            }
            return Ok(());
        }
        if matches!(
            (&left, &right),
            (TypeDescriptor::Dict(_), TypeDescriptor::Struct(_))
        ) {
            return Err(format!(
                "cannot unify {} with {}",
                left.display_name(),
                right.display_name()
            ));
        }
        if !contains_type_variable(&left)
            && !contains_type_variable(&right)
            && (contains_named_type(&left) || contains_named_type(&right))
        {
            return self.check(&left, &right);
        }
        if !contains_type_variable(&left)
            && !contains_type_variable(&right)
            && (assignable(&left, &right) || assignable(&right, &left))
        {
            return Ok(());
        }
        match (&left, &right) {
            (TypeDescriptor::Inference(left), TypeDescriptor::Inference(right))
                if left == right =>
            {
                Ok(())
            }
            (TypeDescriptor::Inference(variable), ty)
            | (ty, TypeDescriptor::Inference(variable)) => {
                self.bind_inference_variable(*variable, ty)
            }
            (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => Ok(()),
            (TypeDescriptor::TypeOf(_), TypeDescriptor::Type) => Ok(()),
            (TypeDescriptor::TypeOf(left), TypeDescriptor::TypeOf(right)) => {
                match (left.as_ref(), right.as_ref()) {
                    (TypeDescriptor::Declared(declared), other)
                        if !matches!(other, TypeDescriptor::Declared(_)) =>
                    {
                        self.unify(&declared.body, other)
                    }
                    (other, TypeDescriptor::Declared(declared))
                        if !matches!(other, TypeDescriptor::Declared(_)) =>
                    {
                        self.unify(other, &declared.body)
                    }
                    _ => self.unify(left, right),
                }
            }
            (TypeDescriptor::Declared(left), TypeDescriptor::Declared(right))
                if left.id.has_same_head(&right.id)
                    && left.id.arguments().len() == right.id.arguments().len() =>
            {
                for (left, right) in left.id.arguments().iter().zip(right.id.arguments()) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (TypeDescriptor::Array(left), TypeDescriptor::Array(right)) => self.unify(left, right),
            (TypeDescriptor::Dict(left), TypeDescriptor::Dict(right)) => self.unify(left, right),
            (
                TypeDescriptor::Tagged {
                    tag: left_tag,
                    payload: left,
                },
                TypeDescriptor::Tagged {
                    tag: right_tag,
                    payload: right,
                },
            ) if left_tag == right_tag => self.unify(left, right),
            (TypeDescriptor::Tagged { tag, payload }, TypeDescriptor::Enum(variants))
            | (TypeDescriptor::Enum(variants), TypeDescriptor::Tagged { tag, payload }) => variants
                .get(tag.name())
                .and_then(Option::as_deref)
                .ok_or_else(|| format!("Enum has no payload variant '{}", tag.name()))
                .and_then(|expected| self.unify(payload, expected)),
            (TypeDescriptor::Atom(tag), TypeDescriptor::Enum(variants))
            | (TypeDescriptor::Enum(variants), TypeDescriptor::Atom(tag))
                if variants.get(tag.name()).is_some_and(Option::is_none) =>
            {
                Ok(())
            }
            (TypeDescriptor::Union(variants), expected @ TypeDescriptor::Enum(_))
            | (expected @ TypeDescriptor::Enum(_), TypeDescriptor::Union(variants)) => {
                for variant in variants {
                    self.unify(variant, expected)?;
                }
                Ok(())
            }
            (TypeDescriptor::Atom(tag), TypeDescriptor::Function { parameters, result })
            | (TypeDescriptor::Function { parameters, result }, TypeDescriptor::Atom(tag))
                if parameters.len() == 1 =>
            {
                self.unify(
                    &TypeDescriptor::Tagged {
                        tag: tag.clone(),
                        payload: Box::new(parameters[0].clone()),
                    },
                    result,
                )
            }
            (TypeDescriptor::Tuple(left), TypeDescriptor::Tuple(right))
            | (TypeDescriptor::Union(left), TypeDescriptor::Union(right))
                if left.len() == right.len() =>
            {
                for (left, right) in left.iter().zip(right) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (TypeDescriptor::Struct(left), TypeDescriptor::Struct(right))
                if left.keys().eq(right.keys()) =>
            {
                for (name, left) in left {
                    self.unify(left, &right[name])?;
                }
                Ok(())
            }
            (TypeDescriptor::Enum(left), TypeDescriptor::Enum(right))
                if left.keys().eq(right.keys()) =>
            {
                for (name, left) in left {
                    match (left.as_deref(), right[name].as_deref()) {
                        (None, None) => {}
                        (Some(left), Some(right)) => self.unify(left, right)?,
                        _ => {
                            return Err(format!("Enum variant {name} payload shape differs"));
                        }
                    }
                }
                Ok(())
            }
            (
                TypeDescriptor::Function {
                    parameters: left_parameters,
                    result: left_result,
                },
                TypeDescriptor::Function {
                    parameters: right_parameters,
                    result: right_result,
                },
            ) if left_parameters.len() == right_parameters.len() => {
                for (left, right) in left_parameters.iter().zip(right_parameters) {
                    self.unify(left, right)?;
                }
                self.unify(left_result, right_result)
            }
            _ if left == right => Ok(()),
            _ => Err(format!(
                "cannot unify {} with {}",
                left.display_name(),
                right.display_name()
            )),
        }
    }

    fn unify_equality(
        &mut self,
        left: &TypeDescriptor,
        right: &TypeDescriptor,
    ) -> Result<(), String> {
        let left = self.resolve(left);
        let right = self.resolve(right);
        if left == right {
            return Ok(());
        }
        match (&left, &right) {
            (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => Ok(()),
            (TypeDescriptor::Atom(_), TypeDescriptor::Atom(_)) => Ok(()),
            (
                TypeDescriptor::Tagged {
                    tag: left_tag,
                    payload: left,
                },
                TypeDescriptor::Tagged {
                    tag: right_tag,
                    payload: right,
                },
            ) => {
                if left_tag == right_tag {
                    self.unify_equality(left, right)
                } else {
                    Ok(())
                }
            }
            (TypeDescriptor::Array(left), TypeDescriptor::Array(right))
            | (TypeDescriptor::Dict(left), TypeDescriptor::Dict(right)) => {
                self.unify_equality(left, right)
            }
            (TypeDescriptor::Tuple(left), TypeDescriptor::Tuple(right))
                if left.len() == right.len() =>
            {
                for (left, right) in left.iter().zip(right) {
                    self.unify_equality(left, right)?;
                }
                Ok(())
            }
            (TypeDescriptor::Struct(left), TypeDescriptor::Struct(right))
                if left.keys().eq(right.keys()) =>
            {
                for (name, left) in left {
                    self.unify_equality(left, &right[name])?;
                }
                Ok(())
            }
            _ => self.unify(&left, &right),
        }
    }

    fn collect_narrow_enum_variants(
        &self,
        descriptor: &TypeDescriptor,
        variants: &mut Vec<(String, Option<TypeDescriptor>)>,
    ) -> bool {
        match self.expose_named(descriptor) {
            TypeDescriptor::Atom(tag) => {
                variants.push((tag.name().to_owned(), None));
                true
            }
            TypeDescriptor::Tagged { tag, payload } => {
                variants.push((tag.name().to_owned(), Some(*payload)));
                true
            }
            TypeDescriptor::Union(items) => items
                .iter()
                .all(|item| self.collect_narrow_enum_variants(item, variants)),
            _ => false,
        }
    }

    fn enum_assignment_failure(
        &self,
        actual: &TypeDescriptor,
        declared: &DeclaredTypeDescriptor,
    ) -> Option<(EnumInferenceFailure, String)> {
        let TypeDescriptor::Enum(expected_variants) = self.declared_body(declared) else {
            return None;
        };
        let mut actual_variants = Vec::new();
        if !self.collect_narrow_enum_variants(actual, &mut actual_variants)
            || actual_variants.is_empty()
        {
            return None;
        }
        actual_variants.sort_by(|left, right| left.0.cmp(&right.0));
        actual_variants.dedup();
        let expected_name = declared.name.clone();
        for (tag, _) in &actual_variants {
            if !expected_variants.contains_key(tag) {
                return Some((
                    EnumInferenceFailure {
                        kind: EnumInferenceFailureKind::IllegalVariant,
                        expected_name: expected_name.clone(),
                    },
                    format!("variant '{tag} is not part of {expected_name}"),
                ));
            }
        }
        for (tag, actual_payload) in &actual_variants {
            let expected_payload = &expected_variants[tag];
            let mismatch = match (actual_payload, expected_payload) {
                (None, None) => None,
                (None, Some(_)) => Some(format!("variant '{tag} requires a payload")),
                (Some(_), None) => Some(format!("variant '{tag} does not accept a payload")),
                (Some(actual), Some(expected)) => {
                    let actual = erase_declared_identity(actual);
                    let expected = erase_declared_identity(expected);
                    incompatibility_path(&actual, &expected).map(|path| {
                        let actual_leaf = type_at_path(&actual, &path).unwrap_or(&actual);
                        let expected_leaf = type_at_path(&expected, &path).unwrap_or(&expected);
                        let path = display_type_path(&path);
                        format!(
                            "variant '{tag} payload is incompatible with {expected_name}{path}: expected {}, found {}",
                            expected_leaf.display_name(),
                            actual_leaf.display_name()
                        )
                    })
                }
            };
            if let Some(message) = mismatch {
                return Some((
                    EnumInferenceFailure {
                        kind: EnumInferenceFailureKind::Payload,
                        expected_name,
                    },
                    message,
                ));
            }
        }
        let variants = actual_variants
            .iter()
            .map(|(tag, _)| format!("'{tag}"))
            .collect::<Vec<_>>()
            .join(" | ");
        let noun = if actual_variants.len() == 1 {
            "variant"
        } else {
            "variants"
        };
        Some((
            EnumInferenceFailure {
                kind: EnumInferenceFailureKind::MissingContext,
                expected_name: expected_name.clone(),
            },
            format!("value was inferred as narrower {noun} {variants}; expected {expected_name}"),
        ))
    }

    fn check(&mut self, actual: &TypeDescriptor, expected: &TypeDescriptor) -> Result<(), String> {
        let completed_actual = self.complete_declared(actual);
        let completed_expected = self.complete_declared(expected);
        let actual = completed_actual.as_ref().unwrap_or(actual);
        let expected = completed_expected.as_ref().unwrap_or(expected);
        if let (Some(actual), Some(expected)) = (
            self.declared_identity(actual),
            self.declared_identity(expected),
        ) && actual.has_same_head(&expected)
            && actual.arguments().len() == expected.arguments().len()
        {
            for (actual, expected) in actual.arguments().iter().zip(expected.arguments()) {
                self.check(actual, expected)?;
            }
            return Ok(());
        }
        match (actual, expected) {
            (TypeDescriptor::Declared(declared), other)
                if !matches!(other, TypeDescriptor::Declared(_))
                    && declared.body.as_ref() == other =>
            {
                return Ok(());
            }
            (other, TypeDescriptor::Declared(declared))
                if !matches!(other, TypeDescriptor::Declared(_))
                    && other == declared.body.as_ref() =>
            {
                return Ok(());
            }
            _ => {}
        }
        if let TypeDescriptor::Declared(actual) = actual
            && !matches!(expected, TypeDescriptor::Declared(_))
        {
            if matches!(expected, TypeDescriptor::Inference(_)) {
                return self.unify(expected, &TypeDescriptor::Declared(actual.clone()));
            }
            return self.check(&actual.body, expected);
        }
        if let TypeDescriptor::Declared(expected) = expected
            && !matches!(actual, TypeDescriptor::Declared(_))
        {
            if matches!(actual, TypeDescriptor::Any | TypeDescriptor::Never) {
                return Ok(());
            }
            if matches!(actual, TypeDescriptor::Inference(_)) {
                return self.unify(actual, &TypeDescriptor::Declared(expected.clone()));
            }
            if let Some((failure, message)) = self.enum_assignment_failure(actual, expected) {
                self.enum_failure = Some(failure);
                return Err(message);
            }
            return Err(format!(
                "cannot unify {} with {}",
                actual.display_name(),
                expected.name
            ));
        }
        if let (TypeDescriptor::Named(actual), TypeDescriptor::Named(expected)) = (actual, expected)
        {
            if actual == expected {
                return Ok(());
            }
            let pair = (actual.clone(), expected.clone());
            if !self.checking_named_pairs.insert(pair.clone()) {
                return Ok(());
            }
            let actual_body = self.named_type(actual).cloned();
            let expected_body = self.named_type(expected).cloned();
            let result = match (actual_body, expected_body) {
                (Some(actual), Some(expected)) => self.check(&actual, &expected),
                _ => Err(format!(
                    "cannot unify {} with {}",
                    display_named_type(actual),
                    display_named_type(expected)
                )),
            };
            self.checking_named_pairs.remove(&pair);
            return result;
        }
        if let Some(actual_name) = self.named_identity(actual) {
            let pair = (actual_name.clone(), format!("{:?}", expected));
            if !self.checking_named_pairs.insert(pair.clone()) {
                return Ok(());
            }
            let actual_body = self.named_type(&actual_name).cloned();
            let result = actual_body.map_or_else(
                || {
                    Err(format!(
                        "unknown concrete type {}",
                        display_named_type(&actual_name)
                    ))
                },
                |actual| self.check(&actual, expected),
            );
            self.checking_named_pairs.remove(&pair);
            return result;
        }
        if let Some(expected_name) = self.named_identity(expected) {
            let pair = (format!("{:?}", actual), expected_name.clone());
            if !self.checking_named_pairs.insert(pair.clone()) {
                return Ok(());
            }
            let expected_body = self.named_type(&expected_name).cloned();
            let result = expected_body.map_or_else(
                || {
                    Err(format!(
                        "unknown concrete type {}",
                        display_named_type(&expected_name)
                    ))
                },
                |expected| self.check(actual, &expected),
            );
            self.checking_named_pairs.remove(&pair);
            return result;
        }
        let actual = self.expose_named(actual);
        let expected = self.expose_named(expected);
        if matches!(actual, TypeDescriptor::Never) {
            return Ok(());
        }
        if let TypeDescriptor::Inference(variable) = expected
            && contains_runtime_never_leaf(&actual)
        {
            let evidence = self.freshen_runtime_never_leaves(&actual);
            return self.bind_inference_variable(variable, &evidence);
        }
        if contains_type_variable(&actual)
            && let TypeDescriptor::Union(variants) = &expected
        {
            let candidates = variants
                .iter()
                .filter(|variant| potentially_assignable(&actual, variant))
                .collect::<Vec<_>>();
            if let [candidate] = candidates.as_slice() {
                return self.check(&actual, candidate);
            }
        }
        match (&actual, &expected) {
            (TypeDescriptor::Atom(_), TypeDescriptor::AtomValue) => return Ok(()),
            (TypeDescriptor::Union(variants), TypeDescriptor::Enum(_)) => {
                for variant in variants {
                    self.check(variant, &expected)?;
                }
                return Ok(());
            }
            (TypeDescriptor::Tagged { tag, payload }, TypeDescriptor::Enum(variants)) => {
                let Some(Some(expected_payload)) = variants.get(tag.name()) else {
                    return Err(format!(
                        "cannot unify {} with {}",
                        actual.display_name(),
                        expected.display_name()
                    ));
                };
                return self.check(payload, expected_payload);
            }
            (TypeDescriptor::Atom(tag), TypeDescriptor::Enum(variants)) => {
                if matches!(variants.get(tag.name()), Some(None)) {
                    return Ok(());
                }
            }
            (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected))
            | (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected))
            | (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
                return self.check(actual, expected);
            }
            (
                TypeDescriptor::Tagged {
                    tag: actual_tag,
                    payload: actual,
                },
                TypeDescriptor::Tagged {
                    tag: expected_tag,
                    payload: expected,
                },
            ) if actual_tag == expected_tag => return self.check(actual, expected),
            (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected))
                if actual.len() == expected.len() =>
            {
                for (actual, expected) in actual.iter().zip(expected) {
                    self.check(actual, expected)?;
                }
                return Ok(());
            }
            (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected))
                if actual.keys().eq(expected.keys()) =>
            {
                for (name, actual) in actual {
                    self.check(actual, &expected[name])?;
                }
                return Ok(());
            }
            (TypeDescriptor::Struct(actual), TypeDescriptor::Dict(expected)) => {
                for actual in actual.values() {
                    self.check(actual, expected)?;
                }
                return Ok(());
            }
            (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected))
                if actual.keys().eq(expected.keys()) =>
            {
                for (name, actual_payload) in actual {
                    match (actual_payload, &expected[name]) {
                        (None, None) => {}
                        (Some(actual), Some(expected)) => self.check(actual, expected)?,
                        _ => {
                            return Err(format!("Enum variant {name} payload shape differs"));
                        }
                    }
                }
                return Ok(());
            }
            (
                TypeDescriptor::Function {
                    parameters: actual_parameters,
                    result: actual_result,
                },
                TypeDescriptor::Function {
                    parameters: expected_parameters,
                    result: expected_result,
                },
            ) if actual_parameters.len() == expected_parameters.len() => {
                for (actual, expected) in actual_parameters.iter().zip(expected_parameters) {
                    self.check(actual, expected)?;
                }
                self.check(actual_result, expected_result)?;
                return Ok(());
            }
            _ => {}
        }
        if !contains_type_variable(&actual) && !contains_type_variable(&expected) {
            return assignable(&actual, &expected).then_some(()).ok_or_else(|| {
                format!(
                    "cannot unify {} with {}",
                    actual.display_name(),
                    expected.display_name()
                )
            });
        }
        self.unify(&actual, &expected)
    }

    fn default_inference_variables_to_any(&mut self, ty: &TypeDescriptor) {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.substitutions.insert(variable, TypeDescriptor::Any);
            }
            TypeDescriptor::Array(item)
            | TypeDescriptor::Dict(item)
            | TypeDescriptor::TypeOf(item) => {
                self.default_inference_variables_to_any(&item);
            }
            TypeDescriptor::Tagged { payload, .. } => {
                self.default_inference_variables_to_any(&payload);
            }
            TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
                for item in items {
                    self.default_inference_variables_to_any(&item);
                }
            }
            TypeDescriptor::Struct(fields) => {
                for field in fields.values() {
                    self.default_inference_variables_to_any(field);
                }
            }
            TypeDescriptor::Enum(variants) => {
                for payload in variants.values().flatten() {
                    self.default_inference_variables_to_any(payload);
                }
            }
            TypeDescriptor::Function { parameters, result } => {
                for parameter in parameters {
                    self.default_inference_variables_to_any(&parameter);
                }
                self.default_inference_variables_to_any(&result);
            }
            _ => {}
        }
    }

    fn freshen_runtime_never_leaves(&mut self, descriptor: &TypeDescriptor) -> TypeDescriptor {
        match descriptor {
            TypeDescriptor::Never => self.fresh_variable(),
            TypeDescriptor::Array(item) => {
                TypeDescriptor::Array(Box::new(self.freshen_runtime_never_leaves(item)))
            }
            TypeDescriptor::Dict(item) => {
                TypeDescriptor::Dict(Box::new(self.freshen_runtime_never_leaves(item)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.freshen_runtime_never_leaves(payload)),
            },
            TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
                items
                    .iter()
                    .map(|item| self.freshen_runtime_never_leaves(item))
                    .collect(),
            ),
            TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
                fields
                    .iter()
                    .map(|(name, field)| (name.clone(), self.freshen_runtime_never_leaves(field)))
                    .collect(),
            ),
            descriptor => descriptor.clone(),
        }
    }

    fn project_field(
        &mut self,
        receiver: &TypeDescriptor,
        field: &str,
    ) -> Result<TypeDescriptor, String> {
        match self.expose_named(receiver) {
            TypeDescriptor::Declared(declared) => self.project_field(&declared.body, field),
            TypeDescriptor::Struct(fields) => fields
                .get(field)
                .cloned()
                .ok_or_else(|| format!("Struct has no field {field:?}")),
            TypeDescriptor::Dict(item) => Ok(*item),
            TypeDescriptor::Union(variants) => variants
                .iter()
                .map(|variant| self.project_field(variant, field))
                .collect::<Result<Vec<_>, _>>()
                .map(join_all_types),
            TypeDescriptor::Never => Ok(TypeDescriptor::Never),
            TypeDescriptor::Any => Ok(TypeDescriptor::Any),
            TypeDescriptor::Inference(variable) => {
                if let Some(result) = self
                    .field_requirements
                    .get(&variable)
                    .and_then(|fields| fields.get(field))
                {
                    return Ok(result.clone());
                }
                let result = self.fresh_variable();
                self.field_requirements
                    .entry(variable)
                    .or_default()
                    .insert(field.to_owned(), result.clone());
                Ok(result)
            }
            descriptor => Err(format!(
                "cannot access field {field:?} on {}",
                descriptor.display_name()
            )),
        }
    }

    fn expose_pattern_type(&self, descriptor: &TypeDescriptor) -> TypeDescriptor {
        match self.expose_named(descriptor) {
            TypeDescriptor::Declared(declared) => self.expose_pattern_type(&declared.body),
            descriptor => descriptor,
        }
    }

    fn project_tuple(
        &mut self,
        receiver: &TypeDescriptor,
        index: usize,
    ) -> Result<TypeDescriptor, String> {
        match self.expose_named(receiver) {
            TypeDescriptor::Tuple(items) => items.get(index).cloned().ok_or_else(|| {
                format!(
                    "Tuple of length {} has no item at index {index}",
                    items.len()
                )
            }),
            TypeDescriptor::Union(variants) => variants
                .iter()
                .map(|variant| self.project_tuple(variant, index))
                .collect::<Result<Vec<_>, _>>()
                .map(canonical_union),
            TypeDescriptor::Never => Ok(TypeDescriptor::Never),
            TypeDescriptor::Any => Ok(TypeDescriptor::Any),
            descriptor => Err(format!(
                "cannot project tuple item {index} from {}",
                descriptor.display_name()
            )),
        }
    }

    fn materialize_field_requirements(
        &mut self,
        descriptor: &TypeDescriptor,
    ) -> Result<(), String> {
        let mut variables = Vec::new();
        collect_inference_variables(&self.resolve(descriptor), &mut variables);
        variables.sort_unstable();
        variables.dedup();
        for variable in variables {
            let Some(fields) = self.field_requirements.remove(&variable) else {
                continue;
            };
            self.bind_inference_variable(variable, &TypeDescriptor::Struct(fields))?;
        }
        Ok(())
    }

    fn narrow_value_origin(&self, expression: &Expr) -> crate::Location {
        if !matches!(expression.value, ExprKind::Variable(_)) {
            return expression.location;
        }
        self.hir
            .expression_ids_at(expression.location)
            .filter_map(|id| self.hir.expression(id))
            .filter_map(|expression| expression.reference)
            .filter_map(|id| self.hir.reference(id))
            .find_map(|reference| match reference.resolution {
                HirResolution::Definition(id) => self
                    .hir
                    .definition(id)
                    .and_then(|definition| definition.value)
                    .and_then(|value| self.hir.expression(value))
                    .map(|expression| expression.location),
                HirResolution::External | HirResolution::Unresolved => None,
            })
            .unwrap_or(expression.location)
    }

    fn direct_enum_failure(
        &self,
        expression: &Expr,
        declared: &DeclaredTypeDescriptor,
        inner_message: &str,
    ) -> Option<(EnumInferenceFailure, String)> {
        let TypeDescriptor::Enum(variants) = self.declared_body(declared) else {
            return None;
        };
        let tag = match &expression.value {
            ExprKind::Atom(name) => name.as_str(),
            ExprKind::Call { callee, .. } => match &callee.value {
                ExprKind::Atom(name) => name.as_str(),
                _ => return None,
            },
            _ => return None,
        };
        let expected_name = declared.name.clone();
        if !variants.contains_key(tag) {
            return Some((
                EnumInferenceFailure {
                    kind: EnumInferenceFailureKind::IllegalVariant,
                    expected_name: expected_name.clone(),
                },
                format!("variant '{tag} is not part of {expected_name}"),
            ));
        }
        let type_failure = inner_message.contains("cannot unify")
            || inner_message.contains("not assignable")
            || inner_message.contains("payload");
        type_failure.then(|| {
            (
                EnumInferenceFailure {
                    kind: EnumInferenceFailureKind::Payload,
                    expected_name: expected_name.clone(),
                },
                format!(
                    "variant '{tag} payload is incompatible with {expected_name}: {inner_message}"
                ),
            )
        })
    }
}
