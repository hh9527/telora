impl Vm {
    pub async fn execute_run(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        mode: crate::codegen::RunMode,
        args: &[String],
        input_sources: &crate::EntryDataSources,
        host: &mut dyn crate::RunHost,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<crate::RunOutcome, String> {
        let result = self
            .execute_run_inner(
                entry,
                mode,
                args,
                input_sources,
                host,
                quota,
                limits,
                sources,
            )
            .await;
        let finished = host.finish().await;
        match (result, finished) {
            (Ok(result), Ok(())) => Ok(result),
            (_, Err(error)) | (Err(error), Ok(())) => Err(error),
        }
    }

    async fn execute_run_inner(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        mode: crate::codegen::RunMode,
        args: &[String],
        input_sources: &crate::EntryDataSources,
        host: &mut dyn crate::RunHost,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<crate::RunOutcome, String> {
        let protocol = entry
            .run_calls
            .as_ref()
            .and_then(|calls| calls.protocol)
            .ok_or("missing statically compiled host protocol types")?;
        let mut session = SolvedRunSession::start(self, entry, quota, limits, sources)?;
        let env = session.host_env(mode, args, input_sources, &host.ees_actors(), protocol)?;
        let caps_value = session.configure(self, env)?;
        let caps = session.host_caps(caps_value, protocol.value)?;
        host.configure(caps.clone())
            .await
            .map_err(|e| format!("cannot satisfy Entry capabilities: {e}"))?;
        let mut fields = vec![];
        for (key, request) in &caps.data_sources {
            if let Some(text) = host.read_data_source(request, limits.file_size).await? {
                let value = session.host_data(
                    crate::EvalSource {
                        source_name: request.src.clone(),
                        format: request.format,
                        text,
                    },
                    protocol.value,
                    limits,
                    sources,
                )?;
                let src = session.host_string(&request.src)?;
                let item =
                    session.host_record(vec![("data".into(), value), ("src".into(), src)])?;
                fields.push((key.clone(), item));
            }
        }
        let prepared = session.host_record(fields)?;
        session.initialize_with_provider(
            self,
            host.resources_provider(),
            prepared,
            protocol.value,
        )?;
        let mut next = None;
        let mut output = String::new();
        loop {
            let event = session.host_event(next, protocol.value, limits, sources)?;
            let effects = session.reduce(self, event)?;
            let count = session
                .host_ref(effects)?
                .sequence_len()
                .ok_or("invalid SystemEffect array")?;
            // Check declared actors and terminal ordering before host dispatch.
            for index in 0..count {
                let effect = session
                    .host_ref(effects)?
                    .sequence_get(index)
                    .ok_or("missing effect")?;
                let (tag, payload) = effect.tagged_parts().ok_or("invalid SystemEffect")?;
                match tag.as_atom().as_deref() {
                    Some("EesCall") => {
                        let actor = host_text(host_field(payload, "actor")?)?;
                        if !caps.ees.contains_key(&actor) {
                            return Err(format!(
                                "Entry emitted an EES effect for undeclared actor {actor:?}"
                            ));
                        }
                    }
                    Some("Output") => {
                        payload.as_str().ok_or("Output payload must be String")?;
                    }
                    Some("Exit") if index + 1 == count && payload.as_int().is_some() => {}
                    Some("Exit") => {
                        return Err("Entry returned an effect after a terminal effect".into());
                    }
                    _ => return Err("invalid SystemEffect".into()),
                }
            }
            for index in 0..count {
                let effect = session
                    .host_ref(effects)?
                    .sequence_get(index)
                    .ok_or("missing effect")?;
                let (tag, payload) = effect.tagged_parts().ok_or("invalid SystemEffect")?;
                match tag.as_atom().as_deref() {
                    Some("EesCall") => {
                        let call = crate::EesCall {
                            key: host_text(host_field(payload, "key")?)?,
                            actor: host_text(host_field(payload, "actor")?)?,
                            operation: host_text(host_field(payload, "operation")?)?,
                            input: session.host_json(
                                host_field(payload, "input")?.runtime(),
                                protocol.value,
                            )?,
                        };
                        if call.key.is_empty() || call.actor.is_empty() || call.operation.is_empty()
                        {
                            return Err(
                                "EesCall key, actor, and operation must not be empty".into()
                            );
                        }
                        host.ees_call(call).await?;
                    }
                    Some("Output") => output.push_str(
                        payload
                            .as_str()
                            .ok_or("Output payload must be String")?
                            .as_str(),
                    ),
                    Some("Exit") => {
                        return Ok(crate::RunOutcome {
                            output,
                            termination: crate::RunTermination::Exit(
                                payload.as_int().ok_or("invalid Exit")?,
                            ),
                        });
                    }
                    _ => unreachable!("validated effect"),
                }
            }
            next = Some(
                host.next_event()
                    .await?
                    .ok_or("Entry made no progress and the Host has no pending event")?,
            );
        }
    }
}

impl SolvedRunSession {
    fn host_ref(&self, value: Val) -> Result<ValueRef<'_>, String> {
        let world = self.world.as_ref().ok_or("failed run session")?;
        Ok(ValueRef::work(value, &world.heap, &self.main))
    }
    fn host_string(&mut self, text: &str) -> Result<Val, String> {
        self.account
            .charge_allocation(text.len() as u64)
            .map_err(|_| "host input allocation quota exceeded")?;
        Ok(Val::unknown(
            self.world
                .as_mut()
                .ok_or("failed run session")?
                .heap
                .string(Some(&self.main), text),
        ))
    }
    fn host_record(&mut self, fields: Vec<(String, Val)>) -> Result<Val, String> {
        solved_eval_record(
            &mut self.world.as_mut().ok_or("failed run session")?.heap,
            &mut self.account,
            fields,
        )
    }
    fn host_array(&mut self, values: Vec<Val>) -> Result<Val, String> {
        self.account
            .charge_allocation(logical_value_bytes(values.len()).map_err(|e| e.message)?)
            .map_err(|_| "host input allocation quota exceeded")?;
        Ok(Val::unknown(DecodedValue::Array(
            self.world
                .as_mut()
                .ok_or("failed run session")?
                .heap
                .allocate(Object::Array(values.into())),
        )))
    }
    fn host_variant(
        &mut self,
        ty: Option<crate::mir::TypeId>,
        name: &str,
        payload: Option<Val>,
    ) -> Result<Val, String> {
        if let Some(ty) = ty {
            let types = self
                .main
                .solved_types
                .as_ref()
                .ok_or("missing type image")?;
            let crate::mir::TypeConstructor::Nominal(symbol) = types.types[ty.index()].constructor
            else {
                return Err("invalid protocol enum identity".into());
            };
            if !types.definition(symbol).is_some_and(|d| {
                d.members
                    .iter()
                    .any(|m| m.name == name && m.payload.is_some() == payload.is_some())
            }) {
                return Err("invalid protocol enum variant".into());
            }
        }
        self.account
            .charge_allocation(
                logical_value_bytes(if payload.is_some() { 2 } else { 0 })
                    .map_err(|e| e.message)?
                    + name.len() as u64,
            )
            .map_err(|_| "host input allocation quota exceeded")?;
        let heap = &mut self.world.as_mut().ok_or("failed run session")?.heap;
        let tag = Val::unknown(heap.atom(Some(&self.main), name));
        let value = if let Some(payload) = payload {
            Val::unknown(DecodedValue::Tagged(
                heap.allocate(Object::Tagged { tag, payload }),
            ))
        } else {
            tag
        };
        Ok(ty.map_or(value, |ty| value.with_type_id(crate::TypeId::solved(ty))))
    }
    fn host_data(
        &mut self,
        source: crate::EvalSource,
        ty: crate::mir::TypeId,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<Val, String> {
        solved_data_value(
            &mut self.world.as_mut().ok_or("failed run session")?.heap,
            Some(&self.main),
            ty,
            source,
            limits,
            sources,
            &mut self.account,
        )
    }
    fn host_json(&self, value: Val, ty: crate::mir::TypeId) -> Result<serde_json::Value, String> {
        let view = HeapView {
            current: &self.world.as_ref().ok_or("failed run session")?.heap,
            background: Some(&self.main),
        };
        serde_json::from_str(&write_solved_json(view, value, ty, None)?).map_err(|e| e.to_string())
    }
    fn host_env(
        &mut self,
        mode: crate::codegen::RunMode,
        args: &[String],
        inputs: &crate::EntryDataSources,
        ees: &std::collections::BTreeMap<String, String>,
        types: crate::codegen::RunHostTypes,
    ) -> Result<Val, String> {
        let mut values = vec![];
        for arg in args {
            values.push(self.host_string(arg)?);
        }
        let args = self.host_array(values)?;
        let mut fields = vec![];
        for (name, kind) in ees {
            fields.push((name.clone(), self.host_string(kind)?));
        }
        let ees = self.host_record(fields)?;
        let mut fields = vec![];
        for (name, input) in inputs {
            let src = self.host_string(&input.src)?;
            let fmt = self.host_variant(
                Some(types.format),
                match input.format {
                    crate::SystemDataFormat::Json => "Json",
                    crate::SystemDataFormat::Yaml => "Yaml",
                    crate::SystemDataFormat::Toml => "Toml",
                },
                None,
            )?;
            let default = self.host_variant(None, "None", None)?;
            let request = self.host_record(vec![
                ("src".into(), src),
                ("fmt".into(), fmt),
                ("default".into(), default),
            ])?;
            fields.push((name.clone(), request));
        }
        let inputs = self.host_record(fields)?;
        let mode = self.host_variant(
            Some(types.mode),
            match mode {
                crate::codegen::RunMode::Run => "Run",
                crate::codegen::RunMode::Serve => "Serve",
            },
            None,
        )?;
        let os = self.host_string(std::env::consts::OS)?;
        let arch = self.host_string(std::env::consts::ARCH)?;
        let platform = self.host_record(vec![("os".into(), os), ("arch".into(), arch)])?;
        self.host_record(vec![
            ("args".into(), args),
            ("ees".into(), ees),
            ("sources".into(), inputs),
            ("mode".into(), mode),
            ("platform".into(), platform),
        ])
    }
    fn host_event(
        &mut self,
        event: Option<crate::SystemEvent>,
        value_type: crate::mir::TypeId,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<Val, String> {
        let (name, payload) = match event {
            None => ("Initialize", None),
            Some(crate::SystemEvent::StdinLine(line)) => {
                let line = match line {
                    Some(line) => {
                        let value = self.host_string(&line)?;
                        self.host_variant(None, "Some", Some(value))?
                    }
                    None => self.host_variant(None, "None", None)?,
                };
                ("StdinLine", Some(line))
            }
            Some(crate::SystemEvent::EesReply(reply)) => {
                let key = self.host_string(&reply.key)?;
                let result = match reply.result {
                    Ok(value) => {
                        let value = self.host_data(
                            crate::EvalSource {
                                source_name: "<EES reply>".into(),
                                format: crate::SystemDataFormat::Json,
                                text: value.to_string(),
                            },
                            value_type,
                            limits,
                            sources,
                        )?;
                        self.host_variant(None, "Ok", Some(value))?
                    }
                    Err(message) => {
                        let value = self.host_string(&message)?;
                        self.host_variant(None, "Err", Some(value))?
                    }
                };
                (
                    "EesReply",
                    Some(self.host_record(vec![("key".into(), key), ("result".into(), result)])?),
                )
            }
        };
        self.host_variant(Some(self.calls.contract.event), name, payload)
    }
}

fn host_field<'a>(value: ValueRef<'a>, name: &str) -> Result<ValueRef<'a>, String> {
    value
        .dict_get(name)
        .ok_or_else(|| format!("host protocol field {name:?} is missing"))
}
fn host_text(value: ValueRef<'_>) -> Result<String, String> {
    value
        .as_str()
        .map(|v| v.as_str().to_owned())
        .ok_or_else(|| "host protocol expects String".into())
}
fn host_dict(value: ValueRef<'_>) -> Result<std::collections::BTreeMap<String, String>, String> {
    value
        .dict_fields()
        .ok_or("host protocol expects Dict")?
        .into_iter()
        .map(|key| Ok((key.to_owned(), host_text(host_field(value, key)?)?)))
        .collect()
}

impl SolvedRunSession {
    fn host_caps(
        &self,
        value: Val,
        value_type: crate::mir::TypeId,
    ) -> Result<crate::SystemCaps, String> {
        let value = self.host_ref(value)?;
        let data = host_field(value, "data_srcs")?;
        let mut data_sources = std::collections::BTreeMap::new();
        for name in data.dict_fields().ok_or("invalid data_srcs")? {
            let item = host_field(data, name)?;
            let src = host_text(host_field(item, "src")?)?;
            if name.is_empty() || src.is_empty() {
                return Err("data source names and paths must be non-empty".into());
            }
            let format = match host_field(item, "fmt")?.as_atom().as_deref() {
                Some("Json") => crate::SystemDataFormat::Json,
                Some("Yaml") => crate::SystemDataFormat::Yaml,
                Some("Toml") => crate::SystemDataFormat::Toml,
                _ => return Err("invalid data format".into()),
            };
            let default = host_field(item, "default")?;
            let has_default = default.as_atom().as_deref() != Some("None");
            if has_default
                && !default
                    .tagged_parts()
                    .is_some_and(|(tag, _)| tag.as_atom().as_deref() == Some("Some"))
            {
                return Err("invalid data default".into());
            }
            data_sources.insert(
                name.to_owned(),
                crate::SystemDataSource {
                    src,
                    format,
                    has_default,
                },
            );
        }
        let ees = host_dict(host_field(value, "ees")?)?;
        if ees
            .iter()
            .any(|(name, kind)| name.is_empty() || kind.is_empty())
        {
            return Err("EES names and kinds must be non-empty".into());
        }
        let ees_vars = host_dict(host_field(value, "ees_vars")?)?;
        if ees_vars.keys().any(String::is_empty) {
            return Err("EES variable names must be non-empty".into());
        }
        let models = host_field(value, "ees_models")?;
        let mut ees_models = vec![];
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..models.sequence_len().ok_or("invalid EES models")? {
            let model = models.sequence_get(index).ok_or("invalid EES model")?;
            let name = host_text(host_field(model, "name")?)?;
            let kind = host_text(host_field(model, "kind")?)?;
            if ees.get(&name) != Some(&kind) || !seen.insert(name.clone()) {
                return Err("EES model does not match its declaration".into());
            }
            let config = self.host_json(host_field(model, "config")?.runtime(), value_type)?;
            ees_models.push(crate::SystemEesModel { name, kind, config });
        }
        if ees_models.len() != ees.len() {
            return Err("EES models do not match declarations".into());
        }
        let texts = host_field(value, "text_srcs")?;
        let mut text_sources = std::collections::BTreeMap::new();
        for name in texts.dict_fields().ok_or("invalid text sources")? {
            let item = host_field(texts, name)?;
            let src = host_text(host_field(item, "src")?)?;
            if name.is_empty() || src.is_empty() {
                return Err("text source names and paths must be non-empty".into());
            }
            let default = host_field(item, "default")?;
            let default = if default.as_atom().as_deref() == Some("None") {
                None
            } else {
                let (tag, payload) = default.tagged_parts().ok_or("invalid text default")?;
                if tag.as_atom().as_deref() != Some("Some") {
                    return Err("invalid text default".into());
                }
                Some(host_text(payload)?)
            };
            text_sources.insert(name.to_owned(), crate::SystemTextSource { src, default });
        }
        let names = host_field(value, "vars")?;
        let mut vars = vec![];
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..names.sequence_len().ok_or("invalid environment names")? {
            let name = host_text(
                names
                    .sequence_get(index)
                    .ok_or("invalid environment name")?,
            )?;
            if name.is_empty() || !seen.insert(name.clone()) {
                return Err("environment names must be unique and non-empty".into());
            }
            vars.push(name);
        }
        let stdin = match host_field(value, "stdin")?.as_atom().as_deref() {
            Some("Null") => crate::SystemStdin::Null,
            Some("Text") => crate::SystemStdin::Text,
            Some("Lined") => crate::SystemStdin::Lined,
            _ => return Err("invalid stdin mode".into()),
        };
        Ok(crate::SystemCaps {
            data_sources,
            ees,
            ees_models,
            ees_vars,
            text_sources,
            vars,
            stdin,
        })
    }
}
