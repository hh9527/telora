use super::*;

impl Lower<'_, '_> {
    pub(super) fn native_adapter(
        &mut self,
        node: HirId,
        arguments: &[TypeKey],
        data: ir::Value,
    ) -> EmitResult<()> {
        let symbol =
            self.mir.hir_symbols[node.index()].ok_or("native declaration has no stable symbol")?;
        let declaration = &self.mir.symbols[symbol.index()];
        let module = declaration
            .module
            .and_then(|id| self.mir.modules[id.index()].native.as_ref())
            .ok_or("native declaration is not in an admitted ABI module")?;
        // These are ABI export keys of an admitted module, not spellings at a
        // source call site. Renaming an import does not change its linked symbol.
        let (operation, input) = match (module.id, declaration.name.as_str()) {
            (5, "length") => (helpers::ARRAY_LENGTH, TypeConstructor::Array),
            (7, "length") => (helpers::STRING_LENGTH, TypeConstructor::String),
            _ => {
                return Err(format!(
                    "native ABI not implemented: module {}, export {}",
                    module.id, declaration.name
                )
                .into());
            }
        };
        if arguments.len() != 1
            || self.mir.types[arguments[0].index()].constructor != input
            || self.mir.types[self.return_type.index()].constructor != TypeConstructor::Int
        {
            return Err("native ABI does not match its sealed declaration signature".into());
        }
        let count = self.builder.ins().iconst(types::I64, 0);
        let result = self.object(node, operation, self.return_type, data, count)?;
        self.write_return(&result)
    }
}
