//! Read-only rendering of the evidence available when a conflict is produced.
//! This never normalizes slots or reconstructs runtime type descriptors.
use super::*;

#[derive(Clone, Copy)]
enum Reference {
    Slot(TypeSlotId),
    Known(TypeId),
}

impl Solver<'_> {
    pub(super) fn diagnostic_type(&self, slot: TypeSlotId) -> String {
        self.render_evidence(Reference::Slot(slot), 0, &mut 128)
    }

    fn render_evidence(&self, reference: Reference, depth: usize, budget: &mut usize) -> String {
        if *budget == 0 { return "…".into(); }
        *budget -= 1;
        if depth >= 32 { return "…".into(); }
        match reference {
            Reference::Slot(slot) => match self.mir.ty_slots[self.root(slot).index()] {
                TypeState::Structure(id) => {
                    let term = &self.mir.type_terms[id.index()];
                    self.render_shape(&term.constructor, term.arguments.iter().copied().map(Reference::Slot), depth, budget)
                }
                TypeState::Known(id) => self.render_evidence(Reference::Known(id), depth, budget),
                TypeState::Unknown => "?".into(),
                TypeState::Conflicted(_) => "<conflicted>".into(),
                TypeState::ProxyTo(_) => unreachable!("root follows proxies"),
            },
            Reference::Known(id) => {
                let ty = &self.mir.types[id.index()];
                self.render_shape(&ty.constructor, ty.arguments.iter().copied().map(Reference::Known), depth, budget)
            }
        }
    }

    fn render_shape(&self, constructor: &TypeConstructor, children: impl Iterator<Item = Reference>, depth: usize, budget: &mut usize) -> String {
        let mut args = vec![];
        for child in children {
            if *budget == 0 { args.push("…".into()); break; }
            args.push(self.render_evidence(child, depth + 1, budget));
        }
        match constructor {
            TypeConstructor::Tuple | TypeConstructor::TupleLiteral =>
                format!("({}{})", args.join(", "), if args.len() == 1 { "," } else { "" }),
            TypeConstructor::Record(names) => format!("{{{}}}", names.iter().zip(&args)
                .map(|(name, ty)| format!("{name}: {ty}")).collect::<Vec<_>>().join(", ")),
            TypeConstructor::Function => match args.split_last() {
                Some((result, parameters)) => format!("Fn({}) -> {result}", parameters.join(", ")),
                None => "Fn(?) -> ?".into(),
            },
            TypeConstructor::Meta | TypeConstructor::TypeOf => format!("TypeOf({})", args.join(", ")),
            TypeConstructor::Nominal(symbol) | TypeConstructor::Parameter(symbol) => {
                let name = &self.mir.symbols[symbol.index()].name;
                if args.is_empty() { name.clone() } else { format!("{name}({})", args.join(", ")) }
            }
            TypeConstructor::Namespace(module) => format!("module {}", self.mir.modules[module.index()].name),
            TypeConstructor::Native(native) => format!("opaque(native:{}#{})", native.module, native.slot),
            TypeConstructor::TypeFunction(function) => format!("type constructor {function:?}"),
            TypeConstructor::ArrayLiteral => {
                args.dedup();
                format!("Array<{}>", if args.is_empty() { "?".into() } else { args.join(", ") })
            }
            other if args.is_empty() => format!("{other:?}"),
            other => format!("{other:?}<{}>", args.join(", ")),
        }
    }
}
