use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl Mir {
    fn native_property(&self, symbol: SymbolId) -> bool {
        let symbol = &self.symbols[symbol.index()];
        symbol.kind == SymbolKind::Declaration(BindingKind::Native)
            && symbol.name == "property"
            && symbol
                .module
                .and_then(|id| self.modules[id.index()].native.as_ref())
                .is_some_and(|module| module.id == 18)
    }

    /// The marker carrier comes from the admitted native ABI, never a source
    /// spelling such as PropertyAttr. Alias references keep that same identity.
    fn property_attribute_type(&self) -> Option<TypeId> {
        let index =
            (0..self.symbols.len()).find(|&index| self.native_property(SymbolId(index as u32)))?;
        let TypeState::Known(signature) = self.ty_slots[self.symbol_types[index].index()] else {
            return None;
        };
        let factory = self.types.get(signature.index())?;
        if factory.constructor != TypeConstructor::Function || factory.arguments.len() != 2 {
            return None;
        }
        let provider = self.types.get(factory.arguments[1].index())?;
        (provider.constructor == TypeConstructor::Function && provider.arguments.len() == 3)
            .then(|| provider.arguments[2])
    }

    fn is_property_marker(&self, decorator: HirId) -> bool {
        let Some(mut node) = self.hir[decorator.index()]
            .children
            .iter()
            .find(|edge| edge.role == Role::Callee)
            .map(|edge| edge.node)
        else {
            return false;
        };
        let mut seen = BTreeSet::new();
        while seen.insert(node) {
            if let Some(slot) = self.hir[node.index()].resolution
                && let ResolveState::Bound(symbol) = self.resolve_slots[slot.index()]
            {
                if self.native_property(symbol) {
                    return true;
                }
                if let Some(&declaration) = self.symbols[symbol.index()].declarations.last() {
                    node = declaration;
                    continue;
                }
            }
            let role = match self.hir[node.index()].kind {
                HirKind::Binding { .. } | HirKind::TypeAscription => Role::Value,
                HirKind::TypeApply => Role::Callee,
                _ => return false,
            };
            let Some(next) = self.hir[node.index()]
                .children
                .iter()
                .find(|edge| edge.role == role)
                .map(|edge| edge.node)
            else {
                return false;
            };
            node = next;
        }
        false
    }

    fn property_admission(
        &self,
        record: &PropertyRecord,
        attribute: Option<TypeId>,
        capabilities: &BTreeMap<TypeId, PropertyId>,
    ) -> Result<Option<PropertyAdmission>, &'static str> {
        if Some(record.property) == attribute {
            if record.site != PropertySite::Type
                || !record
                    .providers
                    .iter()
                    .all(|&provider| self.is_property_marker(provider))
            {
                return Err("PropertyAttr is reserved for @property capability records");
            }
            return Ok(record.concrete.then_some(PropertyAdmission::Capability));
        }
        if !record.concrete {
            return Ok(None);
        }
        let Some(&capability) = capabilities.get(&record.property) else {
            return Err("property type has no @property capability declaration");
        };
        let targets = match record.site {
            PropertySite::Field(_) => 8 | 16,   // Member | Field
            PropertySite::Variant(_) => 8 | 32, // Member | Variant
            PropertySite::Type => {
                let TypeConstructor::Nominal(symbol) = self.types[record.owner.index()].constructor
                else {
                    return Err("property owner must have a nominal skeleton");
                };
                match self
                    .type_definitions
                    .iter()
                    .find(|definition| definition.symbol == symbol)
                    .map(|definition| definition.operation)
                {
                    Some(TypeOperation::Struct | TypeOperation::Newtype) => 1 | 2, // Type | StructType
                    Some(TypeOperation::Enum) => 1 | 4, // Type | EnumType
                    _ => return Err("property owner must have a nominal skeleton"),
                }
            }
        };
        Ok(Some(PropertyAdmission::Require {
            capability,
            targets,
        }))
    }

    fn property_capabilities(&self, attribute: Option<TypeId>) -> BTreeMap<TypeId, PropertyId> {
        self.properties
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.concrete
                    && record.site == PropertySite::Type
                    && Some(record.property) == attribute
            })
            .map(|(index, record)| (record.owner, PropertyId(index as u32)))
            .collect()
    }

    pub(crate) fn build_property_admissions(&mut self) {
        let attribute = self.property_attribute_type();
        let capabilities = self.property_capabilities(attribute);
        for index in 0..self.properties.len() {
            match self.property_admission(&self.properties[index], attribute, &capabilities) {
                Ok(admission) => self.properties[index].admission = admission,
                Err(message) => self.diagnostics.push(Diagnostic::error(
                    message,
                    self.hir[self.properties[index].providers[0].index()].location,
                )),
            }
        }
    }

    pub(super) fn valid_property_admissions(&self) -> bool {
        let attribute = self.property_attribute_type();
        let capabilities = self.property_capabilities(attribute);
        self.properties.iter().all(|record| {
            self.property_admission(record, attribute, &capabilities)
                .is_ok_and(|expected| expected == record.admission)
        })
    }
}
