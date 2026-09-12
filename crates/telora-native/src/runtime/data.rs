use super::*;
use std::collections::BTreeMap;
use telora_core::{
    data_plan::{DataNodeId, DataPlanNodeKind, DataScalar, ValidatedDataPlan},
    mir::{SealedMir, TypeConstructor, TypeState},
};

/// The semantic data contract is selected from the admitted std/value export,
/// not from a type's spelling or the runtime shape of an incoming document.
pub struct DataContract {
    value: TypeId,
    variants: BTreeMap<String, (u32, Option<TypeId>)>,
}
impl DataContract {
    pub fn from_mir(sealed: &SealedMir<'_>) -> Result<Self> {
        let mir = sealed.mir();
        let module = mir
            .modules
            .iter()
            .position(|m| m.native.as_ref().is_some_and(|n| n.id == 23))
            .ok_or("native semantic Value module is not loaded")?;
        let symbol = mir.exports[module]
            .iter()
            .find(|s| mir.symbols[s.index()].name == "Value")
            .ok_or("native semantic Value export missing")?;
        let TypeState::Known(meta) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else {
            return Err("semantic Value export has no closed type".into());
        };
        if mir.types[meta.index()].constructor != TypeConstructor::Meta {
            return Err("semantic Value export is not a type".into());
        }
        let value = *mir.types[meta.index()]
            .arguments
            .first()
            .ok_or("semantic Value type missing")?;
        let TypeConstructor::Nominal(symbol) = mir.types[value.index()].constructor else {
            return Err("semantic Value is not nominal".into());
        };
        let definition = mir
            .type_definitions
            .iter()
            .find(|d| d.symbol == symbol)
            .ok_or("semantic Value skeleton missing")?;
        let layout = mir.type_layouts[value.index()]
            .as_ref()
            .ok_or("semantic Value layout missing")?;
        let mut variants = BTreeMap::new();
        for (index, member) in definition.members.iter().enumerate() {
            let payload = *layout
                .members
                .get(index)
                .ok_or("semantic Value member type missing")?;
            let expected = match member.name.as_str() {
                "None" | "True" | "False" => None,
                "Int" => Some(TypeConstructor::Int),
                "Float" => Some(TypeConstructor::Float),
                "String" | "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => {
                    Some(TypeConstructor::String)
                }
                "Bytes" => Some(TypeConstructor::Bytes),
                "Array" => Some(TypeConstructor::Array),
                "Object" => Some(TypeConstructor::Dict),
                _ => return Err("unknown semantic Value variant".into()),
            };
            match (payload, expected) {
                (None, None) => {}
                (Some(ty), Some(expected)) if mir.types[ty.index()].constructor == expected => {
                    if matches!(expected, TypeConstructor::Array | TypeConstructor::Dict)
                        && mir.types[ty.index()].arguments != [value]
                    {
                        return Err("semantic Value recursive payload mismatch".into());
                    }
                }
                _ => return Err("semantic Value payload ABI mismatch".into()),
            }
            variants.insert(
                member.name.clone(),
                (index as u32, payload.map(TypeId::try_from).transpose()?),
            );
        }
        if variants.len() != 13 {
            return Err("semantic Value variant set mismatch".into());
        }
        Ok(Self {
            value: TypeId::try_from(value)?,
            variants,
        })
    }
    pub fn value_type(&self) -> TypeId {
        self.value
    }
    fn payload(&self, name: &str) -> Result<TypeId> {
        self.variants
            .get(name)
            .and_then(|v| v.1)
            .ok_or_else(|| "semantic payload type missing".into())
    }
}

impl Runtime {
    pub fn materialize_data(
        &mut self,
        contract: &DataContract,
        plan: &ValidatedDataPlan,
    ) -> Result<Value> {
        let root = plan.root_node().ok_or("data plan has no root")?;
        let mut cache = vec![None; plan.nodes().len()];
        let mut visiting = vec![false; plan.nodes().len()];
        self.data_node(contract, plan, root, &mut cache, &mut visiting, 0)
    }
    fn data_node(
        &mut self,
        contract: &DataContract,
        plan: &ValidatedDataPlan,
        id: DataNodeId,
        cache: &mut [Option<Value>],
        visiting: &mut [bool],
        depth: usize,
    ) -> Result<Value> {
        if depth > 512 {
            return Err("native data nesting limit".into());
        }
        if let Some(value) = &cache[id.index()] {
            return Ok(value.clone());
        }
        if visiting[id.index()] {
            return Err("cyclic data plan".into());
        }
        visiting[id.index()] = true;
        let node = &plan.nodes()[id.index()];
        let loc = crate::abi::Origin::from_loc(Some(node.location)).words();
        let (tag, payload) = match &node.kind {
            DataPlanNodeKind::Scalar(scalar) => match scalar {
                DataScalar::Int(value) => (
                    "Int",
                    Some(self.scalar(contract.payload("Int")?, loc, *value as u64)?),
                ),
                DataScalar::Float(value) => (
                    "Float",
                    Some(self.scalar(contract.payload("Float")?, loc, value.to_bits())?),
                ),
                DataScalar::String(value) => (
                    "String",
                    Some(self.string(contract.payload("String")?, loc, value)?),
                ),
                DataScalar::Bytes(value) => (
                    "Bytes",
                    Some(self.bytes(contract.payload("Bytes")?, loc, value)?),
                ),
                DataScalar::Atom(value) => (value.as_str(), None),
                DataScalar::TaggedString { tag, value } => (
                    tag.as_str(),
                    Some(self.string(contract.payload(tag)?, loc, value)?),
                ),
            },
            DataPlanNodeKind::Array(children) => {
                let mut values = Vec::with_capacity(children.len());
                for &child in children {
                    values.push(self.data_node(
                        contract,
                        plan,
                        child,
                        cache,
                        visiting,
                        depth + 1,
                    )?);
                }
                (
                    "Array",
                    Some(self.array(contract.payload("Array")?, loc, &values)?),
                )
            }
            DataPlanNodeKind::Object(fields) => {
                let mut values = Vec::with_capacity(fields.len());
                for (name, field) in fields {
                    let key = self.string(
                        contract.payload("String")?,
                        crate::abi::Origin::from_loc(Some(field.key_location)).words(),
                        name,
                    )?;
                    let value =
                        self.data_node(contract, plan, field.value, cache, visiting, depth + 1)?;
                    values.push((key, value));
                }
                (
                    "Object",
                    Some(self.dict(contract.payload("Object")?, loc, &values)?),
                )
            }
        };
        let index = contract
            .variants
            .get(tag)
            .ok_or("data tag outside semantic contract")?
            .0;
        let value = self.enum_value(contract.value, loc, index, payload.as_ref())?;
        cache[id.index()] = Some(value.clone());
        visiting[id.index()] = false;
        Ok(value)
    }
}
