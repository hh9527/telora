impl InferenceVariables {

    fn contains_runtime_never_leaf(&self, descriptor: &TypeDescriptor) -> bool {
        let mut descriptors = vec![descriptor];
        let mut slots = Vec::new();
        while let Some(descriptor) = descriptors.pop() {
            match descriptor {
                TypeDescriptor::Never => return true,
                TypeDescriptor::Inference(slot) => slots.push(*slot),
                TypeDescriptor::Declared(declared) => descriptors.push(&declared.body),
                TypeDescriptor::Array(item) | TypeDescriptor::Newtype(item)
                | TypeDescriptor::Dict(item) | TypeDescriptor::Tagged { payload: item, .. } => descriptors.push(item),
                TypeDescriptor::Tuple(items) => descriptors.extend(items),
                TypeDescriptor::Struct(fields) => descriptors.extend(fields.values()),
                _ => {},
            }
        }
        let mut visited = HashSet::new();
        while let Some(slot) = slots.pop() {
            let slot = self.root(slot);
            if !visited.insert(slot) { continue; }
            let Some(id) = self.known(slot) else { continue; };
            match self.constructor(id) {
                InferenceConstructor::Never => return true,
                InferenceConstructor::Declared { .. } => slots.extend(self.arguments(id).last()),
                InferenceConstructor::Array | InferenceConstructor::Newtype
                | InferenceConstructor::Dict | InferenceConstructor::Tagged(_)
                | InferenceConstructor::Tuple | InferenceConstructor::Struct(_) => slots.extend(self.arguments(id)),
                _ => {},
            }
        }
        false
    }

    fn same_slots(&self, left: InferenceVariableId, right: InferenceVariableId) -> bool {
        let mut pending = vec![(left, right)];
        let mut visited = HashSet::new();
        while let Some((left, right)) = pending.pop() {
            let (left, right) = (self.root(left), self.root(right));
            if matches!(self.nodes[left.0 as usize].get(), InferenceNode::Conflicted(_))
                || matches!(self.nodes[right.0 as usize].get(), InferenceNode::Conflicted(_))
            { return false; }
            if left == right || !visited.insert((left, right)) { continue; }
            let (Some(left), Some(right)) = (self.known(left), self.known(right)) else { return false; };
            if self.types[left.0 as usize].constructor != self.types[right.0 as usize].constructor {
                return false;
            }
            let (left, right) = (self.arguments(left), self.arguments(right));
            if left.len() != right.len() { return false; }
            pending.extend(left.iter().copied().zip(right.iter().copied()));
        }
        true
    }

    // Run after constraint solving: structural compatibility and nominal
    // refinement must finish before globally coalescing equal known slots.
    fn canonicalize_slots(&mut self) -> usize {
        let mut merged = 0;
        let mut representatives = HashMap::new();
        for variable in self.postorder_slots() {
            let Some(id) = self.known(variable) else { continue; };
            if matches!(self.constructor(id), InferenceConstructor::Bound(_)
                | InferenceConstructor::Named(_) | InferenceConstructor::Declared { .. }
                | InferenceConstructor::PendingAlternatives) { continue; }
            let key = (self.types[id.0 as usize].constructor,
                self.arguments(id).iter().map(|slot| self.root(*slot)).collect::<Box<[_]>>());
            match representatives.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => { entry.insert(variable); },
                std::collections::hash_map::Entry::Occupied(entry) => {
                    self.set(variable, TypeDescriptor::Inference(*entry.get()));
                    merged += 1;
                },
            }
        }
        for index in 0..self.nodes.len() { self.root(InferenceVariableId(index as u32)); }
        merged
    }

    fn postorder_slots(&self) -> Vec<InferenceVariableId> {
        let mut state = vec![0u8; self.nodes.len()];
        let mut order = Vec::with_capacity(self.nodes.len());
        let mut pending = Vec::new();
        for index in 0..self.nodes.len() {
            pending.push((InferenceVariableId(index as u32), false));
            while let Some((slot, finish)) = pending.pop() {
                let slot = self.root(slot);
                if state[slot.0 as usize] == 2 { continue; }
                if finish {
                    state[slot.0 as usize] = 2;
                    order.push(slot);
                } else {
                    assert_eq!(state[slot.0 as usize], 0, "cyclic inference type graph");
                    state[slot.0 as usize] = 1;
                    pending.push((slot, true));
                    if let Some(id) = self.known(slot) {
                        pending.extend(self.arguments(id).iter().rev().map(|slot| (*slot, false)));
                    }
                }
            }
        }
        order
    }

    fn same_type<'a>(&'a self, left: &'a TypeDescriptor, right: &'a TypeDescriptor) -> bool {
        let mut pending = vec![(left, right)];
        let mut visited = HashSet::new();
        while let Some((left, right)) = pending.pop() {
            if let (TypeDescriptor::Inference(left), TypeDescriptor::Inference(right)) = (left, right) {
                if !self.same_slots(*left, *right) { return false; }
                continue;
            }
            let head = |ty: &'a TypeDescriptor| match ty {
                TypeDescriptor::Inference(id) => self.binding(*id).unwrap_or(ty),
                _ => ty,
            };
            let (left, right) = (head(left), head(right));
            if std::ptr::eq(left, right) { continue; }
            if !visited.insert((left as *const _, right as *const _)) { continue; }
            match (left, right) {
                (TypeDescriptor::Inference(a), TypeDescriptor::Inference(b))
                    if self.root(*a) == self.root(*b) => {},
                (TypeDescriptor::Array(a), TypeDescriptor::Array(b))
                | (TypeDescriptor::Dict(a), TypeDescriptor::Dict(b))
                | (TypeDescriptor::Newtype(a), TypeDescriptor::Newtype(b))
                | (TypeDescriptor::TypeOf(a), TypeDescriptor::TypeOf(b)) => pending.push((a, b)),
                (TypeDescriptor::Tagged { tag: a, payload: x }, TypeDescriptor::Tagged { tag: b, payload: y })
                    if a == b => pending.push((x, y)),
                (TypeDescriptor::Tuple(a), TypeDescriptor::Tuple(b))
                | (TypeDescriptor::PendingAlternatives(a), TypeDescriptor::PendingAlternatives(b))
                    if a.len() == b.len() => pending.extend(a.iter().zip(b)),
                (TypeDescriptor::Struct(a), TypeDescriptor::Struct(b)) if a.keys().eq(b.keys()) => {
                    pending.extend(a.values().zip(b.values()));
                },
                (TypeDescriptor::Enum(a), TypeDescriptor::Enum(b)) if a.keys().eq(b.keys()) => {
                    for (a, b) in a.values().zip(b.values()) {
                        match (a, b) {
                            (Some(a), Some(b)) => pending.push((a, b)),
                            (None, None) => {},
                            _ => return false,
                        }
                    }
                },
                (TypeDescriptor::Function { parameters: a, result: x },
                    TypeDescriptor::Function { parameters: b, result: y }) if a.len() == b.len() => {
                    pending.extend(a.iter().zip(b));
                    pending.push((x, y));
                },
                (TypeDescriptor::Declared(a), TypeDescriptor::Declared(b))
                    if a.name == b.name && a.id.has_same_head(&b.id)
                        && a.id.arguments().len() == b.id.arguments().len() => {
                    pending.extend(a.id.arguments().iter().zip(b.id.arguments()));
                    pending.push((&a.body, &b.body));
                },
                _ if left == right => {},
                _ => return false,
            }
        }
        true
    }



    fn refresh(&mut self, mut pending: Vec<InferenceVariableId>) {
        while let Some(variable) = pending.pop() {
            let root = self.root(variable);
            let previous = self.nodes[root.0 as usize].get();
            if matches!(previous, InferenceNode::Conflicted(_)) { continue; }
            let Some(id) = self.known(root) else { continue; };
            let conflict = self.arguments(id).iter().find_map(|dependency| {
                match self.nodes[self.root(*dependency).0 as usize].get() {
                    InferenceNode::Conflicted(id) => Some(id),
                    _ => None,
                }
            });
            let next = if let Some(id) = conflict {
                InferenceNode::Conflicted(id)
            } else {
                previous
            };
            if next != previous {
                self.nodes[root.0 as usize].set(next);
                pending.extend(self.dependent_variables(self.dependents[root.0 as usize]));
            }
        }
    }
}
