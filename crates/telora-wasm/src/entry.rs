//! Persistent std/entry.Eval contract; only the I/O boundary lives in the host.
use crate::{artifact::Kind, output::Output, session::Session};
use std::collections::BTreeMap;
use telora_core::data_plan::ValidatedDataPlan;

impl Session {
    fn eval_field(&mut self, name: &str) -> Result<(u32, u32), String> {
        if self.manifest.eval_type != Some(self.manifest.entry_type) {
            return Err("eval-with export: expected Eval (std/entry.Eval)".into());
        }
        let root = self.entry()?;
        Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        }
        .field(root as u64, self.manifest.entry_type, name)
    }
    pub fn eval_config(&mut self) -> Result<serde_json::Value, String> {
        let (pointer, ty) = self.eval_field("config")?;
        Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        }
        .json(pointer as u64, ty, 0)
    }
    pub fn eval_with(
        &mut self,
        args: &[String],
        env: &BTreeMap<String, String>,
        sources: &[(String, ValidatedDataPlan)],
    ) -> Result<serde_json::Value, String> {
        let (closure, signature) = self.eval_field("evaluate")?;
        let signature = &self.manifest.types[signature as usize];
        if signature.kind != Kind::Function
            || signature.arguments.len() != 2
            || Some(signature.arguments[1]) != self.manifest.value_type
        {
            return Err("Wasm: Eval.evaluate signature differs from sealed contract".into());
        }
        let context_type = signature.arguments[0];
        let result_type = signature.arguments[1];
        let fields = &self.manifest.types[context_type as usize].fields;
        let field = |name: &str| {
            fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.ty)
                .ok_or_else(|| format!("Wasm: Context.{name} missing"))
        };
        let args_type = field("args")?;
        let env_type = field("env")?;
        let sources_type = field("sources")?;
        let string_type = *self.manifest.types[args_type as usize]
            .arguments
            .first()
            .ok_or("Wasm: Context.args lacks element type")?;
        let args = self.input(
            args_type,
            &serde_json::to_value(args).map_err(|e| e.to_string())?,
            0,
        )?;
        let env = self.input(
            env_type,
            &serde_json::to_value(env).map_err(|e| e.to_string())?,
            0,
        )?;
        let mut sources = sources.iter().collect::<Vec<_>>();
        sources.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        if sources.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("Wasm: duplicate entry source".into());
        }
        let mut values = vec![];
        for (name, plan) in sources {
            let key = self.input(string_type, &name.clone().into(), 0)?;
            let value = self.materialize_data(plan)?;
            values.push((key, value));
        }
        let sources = self.input_dict_values(sources_type, &values)?;
        let context = self.input_record_values(
            context_type,
            &BTreeMap::from([("args", args), ("env", env), ("sources", sources)]),
        )?;
        let arguments = self.allocate(4)?;
        self.write(arguments as usize, &context.to_le_bytes())?;
        let invoke = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        let result = invoke
            .call(&mut self.store, (closure as i32, arguments as i32))
            .map_err(|e| e.to_string())? as u32;
        if result == 0 {
            return Err(self.failure());
        }
        Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        }
        .json(result as u64, result_type, 0)
    }
}
