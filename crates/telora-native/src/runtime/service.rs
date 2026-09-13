//! Host protocol materialization from already-sealed field and variant layouts.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use telora_core::{entry_plan::{RunContract, RunMode}, SystemCaps, SystemDataFormat, SystemDataSource,
    SystemEesModel, SystemStdin, SystemTextSource, EesCall};

pub enum ServiceEffect { Output(String), Exit(i64), EesCall(EesCall) }

impl Runtime {
    fn protocol_field_type(&self, ty: TypeId, name: &str) -> Result<TypeId> {
        let layout = self.layout(ty)?;
        let index = layout.field_names.iter().position(|field| field == name).ok_or_else(|| format!("sealed protocol field {name:?} missing"))?;
        layout.fields.get(index).map(|field| field.0).ok_or("sealed protocol field type missing".into())
    }
    fn protocol_field(&self, value: &Value, name: &str) -> Result<Value> {
        let index = self.layout(value.type_id())?.field_names.iter().position(|field| field == name)
            .ok_or_else(|| format!("sealed protocol field {name:?} missing"))?;
        Ok(self.field(value, index)?.to_owned())
    }
    fn protocol_record(&mut self, ty: TypeId, fields: impl IntoIterator<Item = (&'static str, Value)>) -> Result<Value> {
        let mut fields: BTreeMap<_, _> = fields.into_iter().collect();
        let values = self.layout(ty)?.field_names.iter().map(|name| fields.remove(name.as_str())
            .ok_or_else(|| format!("protocol field {name:?} missing"))).collect::<Result<Vec<_>>>()?;
        if !fields.is_empty() { return Err("unexpected protocol fields".into()); }
        self.aggregate(ty, [0; 3], &values)
    }
    fn protocol_element(&self, ty: TypeId) -> Result<TypeId> {
        self.layout(ty)?.arguments.first().copied().ok_or("protocol element type missing".into())
    }
    fn protocol_payload_type(&self, ty: TypeId, name: &str) -> Result<TypeId> {
        self.layout(ty)?.variants.iter().find(|variant| variant.name == name).and_then(|variant| variant.payload)
            .ok_or_else(|| format!("protocol payload {name:?} missing"))
    }
    fn protocol_text(&self, value: &Value) -> Result<String> { Ok(self.text(value.as_ref())?.as_str().to_owned()) }
    fn protocol_tag(&self, value: &Value) -> Result<&str> { self.variant_name(value.type_id(), self.enum_tag(value)?) }
    fn protocol_pairs(&self, value: &Value) -> Result<Vec<(String, Value)>> {
        (0..self.dict_len(value)?).map(|i| {
            let (key, value) = self.dict_entry(value, i)?;
            Ok((self.text(key)?.as_str().to_owned(), value.to_owned()))
        }).collect()
    }
    fn protocol_strings(&self, value: &Value) -> Result<BTreeMap<String, String>> {
        self.protocol_pairs(value)?.into_iter().map(|(key, value)| Ok((key, self.protocol_text(&value)?))).collect()
    }
    fn protocol_dict(&mut self, ty: TypeId, string: TypeId, pairs: impl IntoIterator<Item = (String, Value)>) -> Result<Value> {
        let pairs = pairs.into_iter().map(|(key, value)| Ok((self.string(string, [0; 3], &key)?, value)))
            .collect::<Result<Vec<_>>>()?;
        self.dict(ty, [0; 3], &pairs)
    }
    fn protocol_string_dict(&mut self, ty: TypeId, string: TypeId, pairs: &BTreeMap<String, String>) -> Result<Value> {
        let values = pairs.iter().map(|(key, value)| Ok((key.clone(), self.string(string, [0; 3], value)?)))
            .collect::<Result<Vec<_>>>()?;
        self.protocol_dict(ty, string, values)
    }

    pub fn service_env(&mut self, contract: RunContract, mode: RunMode, args: &[String], inputs: &telora_core::EntryDataSources,
        actors: &BTreeMap<String, String>) -> Result<Value> {
        let env = TypeId::try_from(contract.env)?;
        let args_ty = self.protocol_field_type(env, "args")?;
        let string = self.protocol_element(args_ty)?;
        let values = args.iter().map(|arg| self.string(string, [0; 3], arg)).collect::<Result<Vec<_>>>()?;
        let args = self.array(args_ty, [0; 3], &values)?;
        let ees = self.protocol_string_dict(self.protocol_field_type(env, "ees")?, string, actors)?;
        let source_ty = self.protocol_field_type(env, "sources")?;
        let item_ty = self.protocol_element(source_ty)?;
        let mut values = vec![];
        for (name, input) in inputs {
            let src = self.string(string, [0; 3], &input.src)?;
            let fmt = self.named_variant(self.protocol_field_type(item_ty, "fmt")?, [0; 3], match input.format {
                SystemDataFormat::Json => "Json", SystemDataFormat::Yaml => "Yaml", SystemDataFormat::Toml => "Toml",
            }, None)?;
            let default = self.named_variant(self.protocol_field_type(item_ty, "default")?, [0; 3], "None", None)?;
            values.push((name.clone(), self.protocol_record(item_ty, [("src", src), ("fmt", fmt), ("default", default)])?));
        }
        let sources = self.protocol_dict(source_ty, string, values)?;
        let mode = self.named_variant(self.protocol_field_type(env, "mode")?, [0; 3], match mode { RunMode::Run => "Run", RunMode::Serve => "Serve" }, None)?;
        let os = self.string(string, [0; 3], std::env::consts::OS)?;
        let arch = self.string(string, [0; 3], std::env::consts::ARCH)?;
        let platform = self.protocol_record(self.protocol_field_type(env, "platform")?, [("os", os), ("arch", arch)])?;
        self.protocol_record(env, [("args", args), ("ees", ees), ("sources", sources), ("mode", mode), ("platform", platform)])
    }

    pub fn service_caps(&self, caps: &Value, data: &DataContract) -> Result<SystemCaps> {
        let mut data_sources = BTreeMap::new();
        for (name, request) in self.protocol_pairs(&self.protocol_field(caps, "data_srcs")?)? {
            let src = self.protocol_text(&self.protocol_field(&request, "src")?)?;
            if name.is_empty() || src.is_empty() { return Err("data source names and paths must be non-empty".into()); }
            let format = match self.protocol_tag(&self.protocol_field(&request, "fmt")?)? {
                "Json" => SystemDataFormat::Json, "Yaml" => SystemDataFormat::Yaml, "Toml" => SystemDataFormat::Toml,
                _ => return Err("invalid data format".into()),
            };
            let has_default = match self.protocol_tag(&self.protocol_field(&request, "default")?)? {
                "Some" => true, "None" => false, _ => return Err("invalid data default".into()),
            };
            data_sources.insert(name, SystemDataSource { src, format, has_default });
        }
        let ees = self.protocol_strings(&self.protocol_field(caps, "ees")?)?;
        if ees.iter().any(|(name, kind)| name.is_empty() || kind.is_empty()) { return Err("EES names and kinds must be non-empty".into()); }
        let ees_vars = self.protocol_strings(&self.protocol_field(caps, "ees_vars")?)?;
        if ees_vars.keys().any(String::is_empty) { return Err("EES variable names must be non-empty".into()); }
        let models = self.protocol_field(caps, "ees_models")?;
        let mut ees_models = vec![];
        let mut seen = BTreeSet::new();
        for index in 0..self.array_len(&models)? {
            let model = self.array_get(&models, index)?.to_owned();
            let name = self.protocol_text(&self.protocol_field(&model, "name")?)?;
            let kind = self.protocol_text(&self.protocol_field(&model, "kind")?)?;
            if ees.get(&name) != Some(&kind) || !seen.insert(name.clone()) { return Err("EES model does not match its declaration".into()); }
            let config = serde_json::from_str(&self.semantic_json(data, &self.protocol_field(&model, "config")?)?).map_err(|e| e.to_string())?;
            ees_models.push(SystemEesModel { name, kind, config });
        }
        if ees_models.len() != ees.len() { return Err("EES models do not match declarations".into()); }
        let mut text_sources = BTreeMap::new();
        for (name, request) in self.protocol_pairs(&self.protocol_field(caps, "text_srcs")?)? {
            let src = self.protocol_text(&self.protocol_field(&request, "src")?)?;
            if name.is_empty() || src.is_empty() { return Err("text source names and paths must be non-empty".into()); }
            let default = self.protocol_field(&request, "default")?;
            let default = match self.enum_payload(&default)? { Some(value) => Some(self.protocol_text(&value.to_owned())?), None => None };
            text_sources.insert(name, SystemTextSource { src, default });
        }
        let names = self.protocol_field(caps, "vars")?;
        let mut vars = vec![];
        let mut seen = BTreeSet::new();
        for index in 0..self.array_len(&names)? {
            let name = self.protocol_text(&self.array_get(&names, index)?.to_owned())?;
            if name.is_empty() || !seen.insert(name.clone()) { return Err("environment names must be unique and non-empty".into()); }
            vars.push(name);
        }
        let stdin = match self.protocol_tag(&self.protocol_field(caps, "stdin")?)? {
            "Null" => SystemStdin::Null, "Text" => SystemStdin::Text, "Lined" => SystemStdin::Lined,
            _ => return Err("invalid stdin mode".into()),
        };
        Ok(SystemCaps { data_sources, ees, ees_models, ees_vars, text_sources, vars, stdin })
    }

    pub fn service_resources(&mut self, contract: RunContract, caps: &Value, mut prepared: BTreeMap<String, Value>,
        texts: &BTreeMap<String, String>, vars: &BTreeMap<String, String>, stdin: Option<&str>) -> Result<Value> {
        let resources = TypeId::try_from(contract.resources)?;
        let vars_ty = self.protocol_field_type(resources, "vars")?;
        let string = self.protocol_element(vars_ty)?;
        let data_ty = self.protocol_field_type(resources, "data")?;
        let item_ty = self.protocol_element(data_ty)?;
        let mut values = vec![];
        for (name, request) in self.protocol_pairs(&self.protocol_field(caps, "data_srcs")?)? {
            let src = self.protocol_field(&request, "src")?;
            let value = match prepared.remove(&name) {
                Some(value) => value,
                None => {
                    let default = self.protocol_field(&request, "default")?;
                    self.enum_payload(&default)?.ok_or_else(|| format!("cannot read data source {:?}: file does not exist", self.protocol_text(&src).unwrap_or_default()))?.to_owned()
                }
            };
            values.push((name, self.protocol_record(item_ty, [("src", src), ("data", value)])?));
        }
        if !prepared.is_empty() { return Err("unexpected prepared data sources".into()); }
        let data = self.protocol_dict(data_ty, string, values)?;
        let texts_ty = self.protocol_field_type(resources, "texts")?;
        let item_ty = self.protocol_element(texts_ty)?;
        let mut values = vec![];
        for (name, request) in self.protocol_pairs(&self.protocol_field(caps, "text_srcs")?)? {
            let src = self.protocol_field(&request, "src")?;
            let text = texts.get(&name).ok_or("missing prepared text source")?;
            let text = self.string(string, [0; 3], text)?;
            values.push((name, self.protocol_record(item_ty, [("src", src), ("data", text)])?));
        }
        let texts = self.protocol_dict(texts_ty, string, values)?;
        let vars = self.protocol_string_dict(vars_ty, string, vars)?;
        let stdin_ty = self.protocol_field_type(resources, "stdin")?;
        let stdin = match stdin {
            Some(text) => { let text = self.string(string, [0; 3], text)?; self.named_variant(stdin_ty, [0; 3], "Some", Some(&text))? },
            None => self.named_variant(stdin_ty, [0; 3], "None", None)?,
        };
        self.protocol_record(resources, [("data", data), ("texts", texts), ("vars", vars), ("stdin", stdin)])
    }

    pub fn service_event(&mut self, contract: RunContract, event: Option<telora_core::SystemEvent>, reply: Option<Value>) -> Result<Value> {
        let ty = TypeId::try_from(contract.event)?;
        match event {
            None => self.named_variant(ty, [0; 3], "Initialize", None),
            Some(telora_core::SystemEvent::StdinLine(line)) => {
                let option = self.protocol_payload_type(ty, "StdinLine")?;
                let value = match line {
                    Some(line) => { let text = self.string(self.protocol_element(option)?, [0; 3], &line)?; self.named_variant(option, [0; 3], "Some", Some(&text))? },
                    None => self.named_variant(option, [0; 3], "None", None)?,
                };
                self.named_variant(ty, [0; 3], "StdinLine", Some(&value))
            }
            Some(telora_core::SystemEvent::EesReply(event)) => {
                let payload = self.protocol_payload_type(ty, "EesReply")?;
                let key = self.string(self.protocol_field_type(payload, "key")?, [0; 3], &event.key)?;
                let result_ty = self.protocol_field_type(payload, "result")?;
                let result = match event.result {
                    Ok(_) => self.named_variant(result_ty, [0; 3], "Ok", Some(&reply.ok_or("EES reply data not materialized")?))?,
                    Err(message) => { let error = self.string(self.protocol_payload_type(result_ty, "Err")?, [0; 3], &message)?; self.named_variant(result_ty, [0; 3], "Err", Some(&error))? },
                };
                let payload = self.protocol_record(payload, [("key", key), ("result", result)])?;
                self.named_variant(ty, [0; 3], "EesReply", Some(&payload))
            }
        }
    }

    pub fn service_effects(&self, effects: &Value, caps: &SystemCaps, data: &DataContract) -> Result<Vec<ServiceEffect>> {
        let count = self.array_len(effects)?;
        let mut found = vec![];
        for index in 0..count {
            let effect = self.array_get(effects, index)?.to_owned();
            let payload = self.enum_payload(&effect)?.ok_or("SystemEffect payload missing")?.to_owned();
            let effect = match self.protocol_tag(&effect)? {
                "Output" => ServiceEffect::Output(self.protocol_text(&payload)?),
                "Exit" if index + 1 == count => ServiceEffect::Exit(self.scalar_bits(payload.as_ref())? as i64),
                "Exit" => return Err("Entry returned an effect after a terminal effect".into()),
                "EesCall" => {
                    let actor = self.protocol_text(&self.protocol_field(&payload, "actor")?)?;
                    if !caps.ees.contains_key(&actor) { return Err(format!("Entry emitted an EES effect for undeclared actor {actor:?}")); }
                    let key = self.protocol_text(&self.protocol_field(&payload, "key")?)?;
                    let operation = self.protocol_text(&self.protocol_field(&payload, "operation")?)?;
                    if key.is_empty() || actor.is_empty() || operation.is_empty() { return Err("EesCall key, actor, and operation must not be empty".into()); }
                    let input = serde_json::from_str(&self.semantic_json(data, &self.protocol_field(&payload, "input")?)?).map_err(|e| e.to_string())?;
                    ServiceEffect::EesCall(EesCall { actor, key, operation, input })
                }
                _ => return Err("invalid SystemEffect".into()),
            };
            found.push(effect);
        }
        Ok(found)
    }
}
