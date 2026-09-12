use super::*;
use codec::{Codec, DecodeFailure, external_names};
use std::collections::BTreeMap;

type SchemaResult<T> = std::result::Result<T, DecodeFailure>;

struct Schema<'a, 'b> {
    codec: &'a mut Codec<'b>,
    contract: DataContract,
    properties: Vec<(String, TypeId)>,
    loc: Location,
    links: BTreeMap<TypeId, usize>,
    definitions: Vec<Option<Value>>,
}

impl Codec<'_> {
    pub(super) fn schema(&mut self, target: TypeId, properties: &Value, loc: Location) -> Result<Option<Value>> {
        let contract = self.runtime()?.data_contract.clone().ok_or("semantic Value contract is not loaded")?;
        let properties = self.property_types(properties)?;
        let mut schema = Schema { codec: self, contract, properties, loc, links: BTreeMap::new(), definitions: vec![] };
        match schema.document(target) {
            Ok(value) => Ok(Some(value)),
            Err(DecodeFailure::Failed) => Ok(None),
            Err(DecodeFailure::Runtime(message) | DecodeFailure::Rejected(message, _)) => Err(message),
            Err(DecodeFailure::Blame(_)) => Err("unexpected schema property rejection".into()),
        }
    }
}

impl Schema<'_, '_> {
    fn wrap(&mut self, tag: &str, payload: Option<&Value>) -> SchemaResult<Value> {
        Ok(self.codec.runtime_mut()?.named_variant(self.contract.value_type(), self.loc, tag, payload)?)
    }
    fn string(&mut self, text: &str) -> SchemaResult<Value> {
        let value = self.codec.runtime_mut()?.string(self.contract.payload("String")?, self.loc, text)?;
        self.wrap("String", Some(&value))
    }
    fn int(&mut self, value: usize) -> SchemaResult<Value> {
        let value = self.codec.runtime_mut()?.scalar(self.contract.payload("Int")?, self.loc, value as u64)?;
        self.wrap("Int", Some(&value))
    }
    fn array(&mut self, values: &[Value]) -> SchemaResult<Value> {
        let value = self.codec.runtime_mut()?.array(self.contract.payload("Array")?, self.loc, values)?;
        self.wrap("Array", Some(&value))
    }
    fn object(&mut self, fields: Vec<(String, Value)>) -> SchemaResult<Value> {
        let mut pairs = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            let key = self.codec.runtime_mut()?.string(self.contract.payload("String")?, self.loc, &name)?;
            pairs.push((key, value));
        }
        let value = self.codec.runtime_mut()?.dict(self.contract.payload("Object")?, self.loc, &pairs)?;
        self.wrap("Object", Some(&value))
    }
    fn kind(&mut self, name: &str) -> SchemaResult<Value> {
        let value = self.string(name)?;
        self.object(vec![("type".into(), value)])
    }
    fn reference(&mut self, index: usize) -> SchemaResult<Value> {
        let value = self.string(&format!("#/$defs/Type{index}"))?;
        self.object(vec![("$ref".into(), value)])
    }
    fn document(&mut self, target: TypeId) -> SchemaResult<Value> {
        let root = self.visit(target, false, 0)?;
        let rt = self.codec.runtime()?;
        let dict = rt.enum_payload(&root)?.ok_or("schema root is not an object")?.to_owned();
        let mut fields = vec![];
        for index in 0..rt.dict_len(&dict)? {
            let (key, value) = rt.dict_entry(&dict, index)?;
            fields.push((rt.text(key)?.as_str().to_owned(), value.to_owned()));
        }
        if !self.definitions.is_empty() {
            let definitions = self.definitions.iter().enumerate().map(|(index, value)| {
                Ok((format!("Type{index}"), value.clone().ok_or("unfinished schema definition")?))
            }).collect::<SchemaResult<Vec<_>>>()?;
            fields.push(("$defs".into(), self.object(definitions)?));
        }
        fields.push(("$schema".into(), self.string("https://json-schema.org/draft/2020-12/schema")?));
        self.object(fields)
    }
    fn visit(&mut self, ty: TypeId, body: bool, depth: usize) -> SchemaResult<Value> {
        if depth > 512 { return Err("native schema nesting limit".into()); }
        let info = self.codec.runtime()?.type_info[ty.index()].kind;
        let (rename, untagged, bridged) = if info == Some("Ref") {
            self.codec.options(ty, &self.properties, crate::abi::Origin::from_words(self.loc)?)?
        } else { (false, false, false) };
        if bridged { return self.kind("string"); }
        if info == Some("Ref") && !body {
            if let Some(&index) = self.links.get(&ty) { return self.reference(index); }
            let index = self.definitions.len();
            self.links.insert(ty, index);
            self.definitions.push(None);
            let value = self.visit(ty, true, depth + 1)?;
            self.definitions[index] = Some(value);
            return self.reference(index);
        }
        let layout = self.codec.runtime()?.layout(ty)?;
        let kind = layout.kind;
        let optional = layout.optional;
        let arguments = layout.arguments.clone();
        let fields = layout.fields.iter().map(|(ty, _)| *ty).collect::<Vec<_>>();
        let names = layout.field_names.clone();
        let variants = layout.variants.iter().map(|v| (v.name.clone(), v.payload)).collect::<Vec<_>>();
        match kind {
            Kind::String => self.kind("string"),
            Kind::Scalar => self.kind(match info { Some("Int") => "integer", Some("Float") => "number", Some("Enum") => "boolean", _ => return Err("unsupported scalar schema type".into()) }),
            Kind::Newtype => self.visit(fields[0], false, depth + 1),
            Kind::Array | Kind::Dict => {
                let name = self.string(if kind == Kind::Array { "array" } else { "object" })?;
                let item = self.visit(arguments[0], false, depth + 1)?;
                self.object(vec![("type".into(), name), (if kind == Kind::Array { "items" } else { "additionalProperties" }.into(), item)])
            }
            Kind::Tuple => {
                let name = self.string("array")?;
                let items = fields.iter().map(|&ty| self.visit(ty, false, depth + 1)).collect::<SchemaResult<Vec<_>>>()?;
                let items = self.array(&items)?;
                let len = self.int(fields.len())?;
                self.object(vec![("type".into(), name), ("prefixItems".into(), items), ("minItems".into(), len.clone()), ("maxItems".into(), len)])
            }
            Kind::Record => {
                let names = external_names(names.into_iter(), rename, "duplicate external field name")?;
                let mut properties = vec![];
                let mut required = vec![];
                for (name, ty) in names.into_iter().zip(fields) {
                    if !self.codec.runtime()?.layout(ty)?.optional { required.push(self.string(&name)?); }
                    properties.push((name, self.visit(ty, false, depth + 1)?));
                }
                let mut output = vec![("type".into(), self.string("object")?), ("properties".into(), self.object(properties)?), ("additionalProperties".into(), self.wrap("False", None)?)];
                if !required.is_empty() { output.push(("required".into(), self.array(&required)?)); }
                self.object(output)
            }
            Kind::Enum if optional => {
                let null = self.kind("null")?;
                let value = self.visit(arguments[0], false, depth + 1)?;
                let choices = self.array(&[null, value])?;
                self.object(vec![("anyOf".into(), choices)])
            }
            Kind::Enum => {
                if untagged && variants.iter().filter(|(_, ty)| ty.is_none()).count() > 1 { return Err("untagged Enum may contain at most one unit variant".into()); }
                let names = external_names(variants.iter().map(|(name, _)| name.clone()), rename, "duplicate external variant name")?;
                let mut choices = vec![];
                for (name, (_, payload)) in names.into_iter().zip(variants) {
                    let value = match (untagged, payload) {
                        (true, None) => self.kind("null")?,
                        (true, Some(ty)) => self.visit(ty, false, depth + 1)?,
                        (false, None) => { let name = self.string(&name)?; self.object(vec![("const".into(), name)])? }
                        (false, Some(ty)) => {
                            let value = self.visit(ty, false, depth + 1)?;
                            let properties = self.object(vec![(name.clone(), value)])?;
                            let name = self.string(&name)?;
                            let required = self.array(&[name])?;
                            let kind = self.string("object")?;
                            let additional = self.wrap("False", None)?;
                            self.object(vec![("type".into(), kind), ("properties".into(), properties), ("required".into(), required), ("additionalProperties".into(), additional)])?
                        }
                    };
                    choices.push(value);
                }
                let choices = self.array(&choices)?;
                self.object(vec![("oneOf".into(), choices)])
            }
            _ => Err(format!("Type {info:?} has no JSON Schema mapping").into()),
        }
    }
}
