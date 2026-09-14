//! Finite first-order matching constraints for an instance dependency.
//! The two sides have fresh variable scopes, even for recursive use of the
//! same declaration. No inference slot or published type is modified here.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Term(bool, TypeId);

struct Match<'a> {
    types: &'a [ResolvedType],
    bindings: BTreeMap<Term, Term>,
}

impl Match<'_> {
    fn root(&self, mut term: Term) -> Term {
        while let Some(&next) = self.bindings.get(&term) { term = next; }
        term
    }

    fn variable(&self, term: Term) -> bool {
        matches!(self.types[term.1.index()].constructor, TypeConstructor::Parameter(_))
    }

    fn occurs(&self, variable: Term, term: Term) -> bool {
        let mut pending = vec![term];
        let mut seen = BTreeSet::new();
        while let Some(term) = pending.pop() {
            let term = self.root(term);
            if term == variable { return true; }
            if !seen.insert(term) { continue; }
            pending.extend(self.types[term.1.index()].arguments.iter().map(|&ty| Term(term.0, ty)));
        }
        false
    }

    fn unify(&mut self, pattern: TypeId, actual: TypeId) -> bool {
        let mut pending = vec![(Term(false, pattern), Term(true, actual))];
        let mut seen = BTreeSet::new();
        while let Some((left, right)) = pending.pop() {
            let (left, right) = (self.root(left), self.root(right));
            if left == right || !seen.insert((left, right)) { continue; }
            let binding = if self.variable(left) { Some((left, right)) }
                else if self.variable(right) { Some((right, left)) } else { None };
            if let Some((variable, term)) = binding {
                if self.occurs(variable, term) { return false; }
                self.bindings.insert(variable, term);
            } else {
                let a = &self.types[left.1.index()];
                let b = &self.types[right.1.index()];
                if a.constructor != b.constructor || a.arguments.len() != b.arguments.len() { return false; }
                pending.extend(a.arguments.iter().zip(&b.arguments).map(|(&a, &b)|
                    (Term(left.0, a), Term(right.0, b))));
            }
        }
        true
    }

    fn ground(&self, term: Term) -> bool {
        let mut pending = vec![term];
        let mut seen = BTreeSet::new();
        while let Some(term) = pending.pop() {
            let term = self.root(term);
            if !seen.insert(term) { continue; }
            if self.variable(term) { return false; }
            pending.extend(self.types[term.1.index()].arguments.iter().map(|&ty| Term(term.0, ty)));
        }
        true
    }
}

/// Return source parameters fixed to ground types by the complete match.
/// Their contributions are constants, not unbounded parameter-flow edges.
pub(super) fn fixed_parameters(types: &[ResolvedType], pattern: TypeId, actual: TypeId)
    -> Option<BTreeSet<SymbolId>>
{
    // Unchecked is idempotent. Ordinary first-order occurs checking would
    // reject T = Unchecked(T), although an already unchecked T can satisfy it.
    // Keep the unconstrained overapproximation for these normalization cases.
    let mut pending = vec![pattern, actual];
    let mut seen = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) { continue; }
        if types[ty.index()].constructor == TypeConstructor::Unchecked { return Some(BTreeSet::new()); }
        pending.extend(types[ty.index()].arguments.iter().copied());
    }
    let mut matched = Match { types, bindings: BTreeMap::new() };
    if !matched.unify(pattern, actual) { return None; }
    let mut fixed = BTreeSet::new();
    let mut pending = vec![actual];
    let mut seen = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) { continue; }
        let shape = &types[ty.index()];
        if let TypeConstructor::Parameter(parameter) = shape.constructor
            && matched.ground(Term(true, ty)) {
            fixed.insert(parameter);
        }
        pending.extend(shape.arguments.iter().copied());
    }
    Some(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_preserves_guards_correlations_and_fresh_recursive_binders() {
        let mut types = vec![];
        let mut ty = |constructor, arguments| {
            let id = TypeId(types.len() as u32);
            types.push(ResolvedType { constructor, arguments });
            id
        };
        let int = ty(TypeConstructor::Int, vec![]);
        let t = ty(TypeConstructor::Parameter(SymbolId(0)), vec![]);
        let u = ty(TypeConstructor::Parameter(SymbolId(1)), vec![]);
        let array_t = ty(TypeConstructor::Array, vec![t]);
        let actual = ty(TypeConstructor::Tuple, vec![t, array_t]);
        let guarded = ty(TypeConstructor::Tuple, vec![int, u]);
        let repeated = ty(TypeConstructor::Tuple, vec![u, u]);
        let same = ty(TypeConstructor::Tuple, vec![t, t]);
        let unchecked = ty(TypeConstructor::Unchecked, vec![t]);
        let idempotent = ty(TypeConstructor::Tuple, vec![t, unchecked]);
        assert_eq!(fixed_parameters(&types, guarded, actual), Some(BTreeSet::from([SymbolId(0)])));
        // A repeated pattern binder is an equality constraint, not two wildcards.
        assert!(fixed_parameters(&types, repeated, actual).is_none());
        assert_eq!(fixed_parameters(&types, repeated, same), Some(BTreeSet::new()));
        // The recursive declaration's T is fresh on each side: T -> Array(T)
        // must remain a possible growth transition, not an occurs-check error.
        assert_eq!(fixed_parameters(&types, t, array_t), Some(BTreeSet::new()));
        assert!(fixed_parameters(&types, repeated, idempotent).is_some());
    }
}
