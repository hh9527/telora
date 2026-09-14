use super::*;

impl Solver<'_> {
    pub(super) fn finalize_value_materializations(&mut self) {
        self.mir.value_materializations = (0..self.mir.hir.len())
            .map(|index| self.mir.materialization_identity(HirId(index as u32)))
            .collect();
    }
}
