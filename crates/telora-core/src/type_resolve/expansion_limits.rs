//! Resource guard for resolved type expansion, not a convergence verdict.
//! Canonical types are appended after their children. Each new node is measured
//! once; nominal member layouts are not traversed as structural type arguments.
use super::*;

pub(super) const MAX_TYPE_DEPTH: usize = 256;
pub(super) const MAX_TUPLE_ITEMS: usize = 1024;
// Also bounds variable-arity internal shapes (records, signatures, type lists).
pub(super) const MAX_TYPE_ARGUMENTS: usize = 4096;

impl Solver<'_> {
    pub(super) fn check_type_expansion(&mut self, origin: Option<Location>) -> bool {
        if self.expansion_exhausted { return false; }
        while self.type_depths.len() < self.mir.types.len() {
            let index = self.type_depths.len();
            let ty = &self.mir.types[index];
            let depth = 1 + ty.arguments.iter().map(|child| {
                self.type_depths[child.index()]
            }).max().unwrap_or(0);
            self.type_depths.push(depth);
            let tuple = matches!(ty.constructor,
                TypeConstructor::Tuple | TypeConstructor::TupleLiteral);
            let message = if tuple && ty.arguments.len() > MAX_TUPLE_ITEMS {
                Some(format!("tuple item limit exceeded (maximum {MAX_TUPLE_ITEMS}); type closure incomplete"))
            } else if ty.arguments.len() > MAX_TYPE_ARGUMENTS {
                Some(format!("type argument limit exceeded (maximum {MAX_TYPE_ARGUMENTS}); type closure incomplete"))
            } else if depth > MAX_TYPE_DEPTH {
                Some(format!("type expansion depth limit exceeded (maximum {MAX_TYPE_DEPTH}); type closure incomplete"))
            } else { None };
            if let Some(message) = message {
                let location = origin.or_else(|| self.mir.hir.iter().enumerate().find_map(|(node, syntax)| {
                    (self.mir.ty_slots[node] == TypeState::Known(TypeId(index as u32)))
                        .then_some(syntax.location)
                })).or_else(|| self.mir.hir.first().map(|node| node.location));
                let mut diagnostic = Diagnostic {
                    severity: crate::source::Severity::Error,
                    message, labels: vec![], notes: vec![],
                };
                if let Some(location) = location {
                    diagnostic = Diagnostic::error(diagnostic.message, location);
                }
                diagnostic.notes.push("This is a static resource limit, not proof of infinite expansion.".into());
                self.mir.diagnostics.push(diagnostic);
                self.expansion_exhausted = true;
                return false;
            }
        }
        true
    }
}
