//! Immutable codegen type data. All edges are solved IDs, never inference slots
//! or recursively owned descriptors. Generic bodies remain parameterized;
//! nominal applications retain their arguments in the ordinary type table.
use crate::{
    mir::{Mir, ResolvedType, SymbolId, TypeConstructor, TypeId, TypeOperation, TypeState},
    source::Diagnostic,
};

/// Native algebraic families have a fixed representation, independent of their
/// type parameters. The type pass has already selected the variant index.
pub(crate) fn builtin_variant(
    constructor: &crate::mir::TypeConstructor,
    index: u32,
) -> Option<(&'static str, bool)> {
    use crate::mir::TypeConstructor as T;
    Some(match (constructor, index) {
        (T::Option, 0) => ("None", false),
        (T::Option, 1) => ("Some", true),
        (T::Result, 0) => ("Ok", true),
        (T::Result, 1) => ("Err", true),
        (T::FoldControl, 0) => ("Continue", true),
        (T::FoldControl, 1) => ("Break", true),
        _ => return None,
    })
}

#[derive(Debug)]
pub struct TypeImage {
    /// Indices are exactly the TypeIds assigned by the static pass.
    pub types: Vec<ResolvedType>,
    pub layouts: Vec<Option<crate::mir::TypeLayout>>,
    pub definitions: Vec<TypeDefinition>,
    pub(crate) native_definitions: Vec<(crate::mir::NativeTypeId, String)>,
    /// Input identity from the admitted JSON formatter's solved ABI signature.
    /// Runtime serializers must not infer this contract from the payload stamp.
    pub(crate) json_value_type: Option<TypeId>,
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
    /// Applied member types in declaration order. This is an array lookup,
    /// including for recursive and generic nominal applications.
    pub fn layout(&self, ty: TypeId) -> Option<&crate::mir::TypeLayout> {
        // Unchecked changes the outer guarantee, not the representation. Its
        // canonical argument is the already solved owner; share that layout.
        let ty = if self.types.get(ty.index())?.constructor == TypeConstructor::Unchecked {
            *self.types[ty.index()].arguments.first()?
        } else { ty };
        self.layouts.get(ty.index())?.as_ref()
    }

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
            layouts: mir.type_layouts.clone(),
            definitions,
            native_definitions: mir
                .symbols
                .iter()
                .filter_map(|symbol| {
                    if symbol.kind
                        != crate::mir::SymbolKind::Declaration(crate::ast::BindingKind::NativeType)
                    {
                        return None;
                    }
                    let id = symbol.native_type?;
                    let module = &mir.modules[symbol.module?.index()].name;
                    Some((id, format!("{module}#{}", symbol.name)))
                })
                .collect(),
            json_value_type: mir.symbols.iter().find_map(|symbol| {
                use crate::{
                    ast::BindingKind,
                    mir::{SymbolKind, TypeConstructor},
                };
                if symbol.kind != SymbolKind::Declaration(BindingKind::Native)
                    || symbol.name != "stringify"
                    || mir.modules[symbol.module?.index()].native.as_ref()?.id != 17
                {
                    return None;
                }
                let TypeState::Known(signature) =
                    mir.ty_slots[symbol.declarations.first()?.ty().index()]
                else {
                    return None;
                };
                let signature = &mir.types[signature.index()];
                (signature.constructor == TypeConstructor::Function
                    && signature.arguments.len() == 2)
                    .then(|| signature.arguments[0])
            }),
            definition_by_symbol,
        })
    }

    pub fn definition(&self, symbol: SymbolId) -> Option<&TypeDefinition> {
        self.definition_by_symbol
            .get(symbol.index())?
            .map(|index| &self.definitions[index])
    }
}
