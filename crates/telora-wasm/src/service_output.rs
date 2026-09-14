//! Only capabilities and effects cross the host boundary; service state stays put.
use crate::{session::Session, transport::Value};
use std::collections::{BTreeMap, BTreeSet};
use telora_core::{
    EesCall, SystemCaps, SystemDataFormat, SystemDataSource, SystemEesModel, SystemStdin,
    SystemTextSource,
};

pub enum ServiceEffect {
    Output(String),
    Exit(i64),
    EesCall(EesCall),
}

impl Session {
    fn string_dict(&self, value: Value) -> Result<BTreeMap<String, String>, String> {
        self.dict_items(value)?
            .into_iter()
            .map(|(name, value)| Ok((name, self.text_value(value)?)))
            .collect()
    }
    pub fn service_caps(&self, caps: Value) -> Result<SystemCaps, String> {
        let mut data_sources = BTreeMap::new();
        for (name, request) in self.dict_items(self.value_field(caps, "data_srcs")?)? {
            let src = self.text_value(self.value_field(request, "src")?)?;
            if name.is_empty() || src.is_empty() {
                return Err("data source names and paths must be non-empty".into());
            }
            let format = match self.variant(self.value_field(request, "fmt")?)?.0 {
                "Json" => SystemDataFormat::Json,
                "Yaml" => SystemDataFormat::Yaml,
                "Toml" => SystemDataFormat::Toml,
                _ => return Err("invalid data format".into()),
            };
            let has_default = match self.variant(self.value_field(request, "default")?)?.0 {
                "Some" => true,
                "None" => false,
                _ => return Err("invalid data default".into()),
            };
            data_sources.insert(
                name,
                SystemDataSource {
                    src,
                    format,
                    has_default,
                },
            );
        }
        let ees = self.string_dict(self.value_field(caps, "ees")?)?;
        if ees.iter().any(|(n, k)| n.is_empty() || k.is_empty()) {
            return Err("EES names and kinds must be non-empty".into());
        }
        let ees_vars = self.string_dict(self.value_field(caps, "ees_vars")?)?;
        if ees_vars.keys().any(String::is_empty) {
            return Err("EES variable names must be non-empty".into());
        }
        let mut ees_models = vec![];
        let mut seen = BTreeSet::new();
        for model in self.array_items(self.value_field(caps, "ees_models")?)? {
            let name = self.text_value(self.value_field(model, "name")?)?;
            let kind = self.text_value(self.value_field(model, "kind")?)?;
            if ees.get(&name) != Some(&kind) || !seen.insert(name.clone()) {
                return Err("EES model does not match its declaration".into());
            }
            let config = self.output_value(self.value_field(model, "config")?)?;
            ees_models.push(SystemEesModel { name, kind, config });
        }
        if ees_models.len() != ees.len() {
            return Err("EES models do not match declarations".into());
        }
        let mut text_sources = BTreeMap::new();
        for (name, request) in self.dict_items(self.value_field(caps, "text_srcs")?)? {
            let src = self.text_value(self.value_field(request, "src")?)?;
            if name.is_empty() || src.is_empty() {
                return Err("text source names and paths must be non-empty".into());
            }
            let default = self
                .variant(self.value_field(request, "default")?)?
                .1
                .map(|v| self.text_value(v))
                .transpose()?;
            text_sources.insert(name, SystemTextSource { src, default });
        }
        let mut vars = vec![];
        let mut seen = BTreeSet::new();
        for value in self.array_items(self.value_field(caps, "vars")?)? {
            let name = self.text_value(value)?;
            if name.is_empty() || !seen.insert(name.clone()) {
                return Err("environment names must be unique and non-empty".into());
            }
            vars.push(name);
        }
        let stdin = match self.variant(self.value_field(caps, "stdin")?)?.0 {
            "Null" => SystemStdin::Null,
            "Text" => SystemStdin::Text,
            "Lined" => SystemStdin::Lined,
            _ => return Err("invalid stdin mode".into()),
        };
        Ok(SystemCaps {
            data_sources,
            ees,
            ees_models,
            ees_vars,
            text_sources,
            vars,
            stdin,
        })
    }
    pub fn service_effects(
        &self,
        effects: Value,
        caps: &SystemCaps,
    ) -> Result<Vec<ServiceEffect>, String> {
        let values = self.array_items(effects)?;
        let mut found = vec![];
        for (index, value) in values.iter().enumerate() {
            let (name, payload) = self.variant(*value)?;
            let payload = payload.ok_or("SystemEffect payload missing")?;
            let effect = match name {
                "Output" => ServiceEffect::Output(self.text_value(payload)?),
                "Exit" if index + 1 == values.len() => ServiceEffect::Exit(
                    self.output_value(payload)?
                        .as_i64()
                        .ok_or("invalid exit code")?,
                ),
                "Exit" => return Err("Entry returned an effect after a terminal effect".into()),
                "EesCall" => {
                    let actor = self.text_value(self.value_field(payload, "actor")?)?;
                    if !caps.ees.contains_key(&actor) {
                        return Err(format!(
                            "Entry emitted an EES effect for undeclared actor {actor:?}"
                        ));
                    }
                    let key = self.text_value(self.value_field(payload, "key")?)?;
                    let operation = self.text_value(self.value_field(payload, "operation")?)?;
                    if key.is_empty() || actor.is_empty() || operation.is_empty() {
                        return Err("EesCall key, actor, and operation must not be empty".into());
                    }
                    let input = self.output_value(self.value_field(payload, "input")?)?;
                    ServiceEffect::EesCall(EesCall {
                        actor,
                        key,
                        operation,
                        input,
                    })
                }
                _ => return Err("invalid SystemEffect".into()),
            };
            found.push(effect);
        }
        Ok(found)
    }
}
