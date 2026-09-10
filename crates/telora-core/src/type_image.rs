//! Immutable codegen type data. All edges are solved IDs, never inference slots
//! or recursively owned descriptors. Generic bodies remain parameterized;
//! nominal applications retain their arguments in the ordinary type table.
use crate::{
    mir::{Mir, ResolvedType, SymbolId, TypeId, TypeOperation, TypeState},
    source::Diagnostic,
};

#[derive(Debug)]
pub struct TypeImage {
    /// Indices are exactly the TypeIds assigned by the static pass.
    pub types: Vec<ResolvedType>,
    pub definitions: Vec<TypeDefinition>,
    definition_by_symbol: Vec<Option<usize>>,
}

#[derive(Debug)]
pub struct TypeDefinition {
    pub symbol: SymbolId,
    pub name: String,
    pub operation: TypeOperation,
    pub parameters: Vec<SymbolId>,
    pub members: Vec<TypeMember>,
}

#[derive(Debug)]
pub struct TypeMember {
    pub name: String,
    /// None means a nullary variant, not an unknown type.
    pub payload: Option<TypeId>,
}

impl TypeImage {
    pub fn variant(&self, ty: TypeId, index: u32) -> Option<&TypeMember> {
        let crate::mir::TypeConstructor::Nominal(symbol) = self.types.get(ty.index())?.constructor
        else {
            return None;
        };
        let definition = self.definition(symbol)?;
        if definition.operation != TypeOperation::Enum {
            return None;
        }
        definition.members.get(index as usize)
    }

    pub(crate) fn from_mir(mir: &Mir) -> Result<Self, Vec<Diagnostic>> {
        let mut diagnostics = vec![];
        let mut definitions = vec![];
        let mut definition_by_symbol = vec![None; mir.symbols.len()];
        for definition in &mir.type_definitions {
            let mut members = vec![];
            for member in &definition.members {
                let payload = match member.payload {
                    None => None,
                    Some(slot) => match mir.ty_slots[slot.index()] {
                        TypeState::Known(id) => Some(id),
                        _ => {
                            diagnostics.push(Diagnostic::error(
                                "type definition member has no normalized TypeId",
                                mir.hir[member.syntax.index()].location,
                            ));
                            continue;
                        }
                    },
                };
                members.push(TypeMember {
                    name: member.name.clone(),
                    payload,
                });
            }
            let symbol = &mir.symbols[definition.symbol.index()];
            let name = match symbol.module {
                Some(module) => format!("{}::{}", mir.modules[module.index()].name, symbol.name),
                None => symbol.name.clone(),
            };
            definition_by_symbol[definition.symbol.index()] = Some(definitions.len());
            definitions.push(TypeDefinition {
                symbol: definition.symbol,
                name,
                operation: definition.operation,
                parameters: definition.parameters.clone(),
                members,
            });
        }
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        Ok(Self {
            types: mir.types.clone(),
            definitions,
            definition_by_symbol,
        })
    }

    pub fn definition(&self, symbol: SymbolId) -> Option<&TypeDefinition> {
        self.definition_by_symbol
            .get(symbol.index())?
            .map(|index| &self.definitions[index])
    }
}
