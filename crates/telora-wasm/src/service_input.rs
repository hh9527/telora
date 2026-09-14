//! External service inputs materialized once into sealed Wasm protocol types.
use crate::{session::Session, transport::Value};
use std::collections::BTreeMap;
use telora_core::{
    SystemDataFormat, SystemEvent,
    entry_plan::{RunContract, RunMode},
};

impl Session {
    pub fn service_env(
        &mut self,
        contract: RunContract,
        mode: RunMode,
        args: &[String],
        inputs: &telora_core::EntryDataSources,
        actors: &BTreeMap<String, String>,
    ) -> Result<Value, String> {
        let sources:serde_json::Map<_,_>=inputs.iter().map(|(name,input)| (name.clone(),serde_json::json!({
            "src":input.src,"fmt":match input.format {SystemDataFormat::Json=>"Json",SystemDataFormat::Yaml=>"Yaml",SystemDataFormat::Toml=>"Toml"},"default":null
        }))).collect();
        self.input_value(contract.env.index() as u32,&serde_json::json!({
            "args":args,"ees":actors,"sources":sources,"mode":match mode {RunMode::Run=>"Run",RunMode::Serve=>"Serve"},
            "platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH}
        }))
    }

    pub fn service_resources(
        &mut self,
        contract: RunContract,
        caps: Value,
        mut prepared: BTreeMap<String, Value>,
        texts: &BTreeMap<String, String>,
        vars: &BTreeMap<String, String>,
        stdin: Option<&str>,
    ) -> Result<Value, String> {
        let resources = contract.resources.index() as u32;
        let vars_ty = self.field_type(resources, "vars")?;
        let string = self.element_type(vars_ty)?;
        let data_ty = self.field_type(resources, "data")?;
        let item_ty = self.element_type(data_ty)?;
        let mut values = vec![];
        for (name, request) in self.dict_items(self.value_field(caps, "data_srcs")?)? {
            let src = self.value_field(request, "src")?;
            let value = match prepared.remove(&name) {
                Some(value) => value,
                None => self
                    .variant(self.value_field(request, "default")?)?
                    .1
                    .ok_or_else(|| {
                        format!(
                            "cannot read data source {:?}: file does not exist",
                            self.text_value(src).unwrap_or_default()
                        )
                    })?,
            };
            values.push((
                name,
                self.record_value(item_ty, [("src", src), ("data", value)])?,
            ));
        }
        if !prepared.is_empty() {
            return Err("unexpected prepared data sources".into());
        }
        let data = self.dict_value(data_ty, string, values)?;
        let texts_ty = self.field_type(resources, "texts")?;
        let item_ty = self.element_type(texts_ty)?;
        let mut values = vec![];
        for (name, request) in self.dict_items(self.value_field(caps, "text_srcs")?)? {
            let src = self.value_field(request, "src")?;
            let text = texts.get(&name).ok_or("missing prepared text source")?;
            let text = self.input_value(string, &text.clone().into())?;
            values.push((
                name,
                self.record_value(item_ty, [("src", src), ("data", text)])?,
            ));
        }
        let texts = self.dict_value(texts_ty, string, values)?;
        let vars = self.input_value(
            vars_ty,
            &serde_json::to_value(vars).map_err(|e| e.to_string())?,
        )?;
        let stdin_ty = self.field_type(resources, "stdin")?;
        let stdin = self.input_value(
            stdin_ty,
            &serde_json::to_value(stdin).map_err(|e| e.to_string())?,
        )?;
        self.record_value(
            resources,
            [
                ("data", data),
                ("texts", texts),
                ("vars", vars),
                ("stdin", stdin),
            ],
        )
    }

    pub fn service_event(
        &mut self,
        contract: RunContract,
        event: Option<SystemEvent>,
        reply: Option<Value>,
    ) -> Result<Value, String> {
        let ty = contract.event.index() as u32;
        match event {
            None => self.named_variant(ty, "Initialize", None),
            Some(SystemEvent::StdinLine(line)) => {
                self.input_value(ty, &serde_json::json!({"StdinLine":line}))
            }
            Some(SystemEvent::EesReply(event)) => {
                let payload = self.variant_type(ty, "EesReply")?;
                let key = self.input_value(self.field_type(payload, "key")?, &event.key.into())?;
                let result_ty = self.field_type(payload, "result")?;
                let result = match event.result {
                    Ok(_) => self.named_variant(
                        result_ty,
                        "Ok",
                        Some(reply.ok_or("EES reply not materialized")?),
                    )?,
                    Err(message) => {
                        let error = self
                            .input_value(self.variant_type(result_ty, "Err")?, &message.into())?;
                        self.named_variant(result_ty, "Err", Some(error))?
                    }
                };
                let payload = self.record_value(payload, [("key", key), ("result", result)])?;
                self.named_variant(ty, "EesReply", Some(payload))
            }
        }
    }
    pub fn materialize_value(
        &mut self,
        plan: &telora_core::data_plan::ValidatedDataPlan,
    ) -> Result<Value, String> {
        let ty = self
            .manifest
            .value_type
            .ok_or("Wasm: missing semantic Value type")?;
        Ok(Value {
            pointer: self.materialize_data(plan)?,
            ty,
        })
    }
}
