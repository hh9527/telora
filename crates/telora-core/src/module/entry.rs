#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModuleSourcePolicy {
    ExplicitExports,
    ExpressionHarness,
}

#[derive(Clone, Copy)]
enum TeloraModuleSource<'a> {
    File(&'a Path),
    Synthetic {
        name: &'a str,
        context_path: &'a Path,
        source: &'a str,
    },
}

impl<'a> TeloraModuleSource<'a> {
    fn context_path(self) -> &'a Path {
        match self {
            Self::File(path)
            | Self::Synthetic {
                context_path: path, ..
            } => path,
        }
    }
}

fn load_module_with_policy(
    path: impl AsRef<Path>,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    module_quota: Quota,
    data_limits: DataLimits,
    debug_sink: Arc<dyn DebugSink>,
    source_policy: ModuleSourcePolicy,
) -> Result<LoadedModule, ModuleError> {
    let resolver = ModuleResolver::for_root(path.as_ref())
        .map_err(|error| ModuleError::new(error.to_string()))?
        .with_builtins(builtin_list());
    load_module_with_resolver(
        resolver,
        external_bindings,
        module_quota,
        data_limits,
        debug_sink,
        source_policy,
    )
}

fn load_module_with_resolver(
    resolver: ModuleResolver,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    module_quota: Quota,
    data_limits: DataLimits,
    debug_sink: Arc<dyn DebugSink>,
    source_policy: ModuleSourcePolicy,
) -> Result<LoadedModule, ModuleError> {
    let root_module = resolver
        .selected_root()
        .map_err(|error| ModuleError::new(error.to_string()))?;
    if root_module.format != ModuleFormat::Telora {
        return Err(ModuleError::new(
            "root module must have a .telora extension",
        ));
    }
    let opaque_modules = builtin_list()
        .into_iter()
        .map(|(name, _)| ModuleCName::builtin(name));
    let graph = ModuleGraph::discover(
        &resolver,
        vec![root_module.clone()],
        &BTreeMap::new(),
        opaque_modules,
        None,
        false,
    )?;
    let mut main = MainWorld::with_modules(graph);
    let mut sources = SourceDatabase::default();
    let builtin_modules = install_native_modules(&mut main, &mut sources, &debug_sink)?;
    let mut loader = ModuleLoader {
        resolver,
        cache: HashMap::new(),
        builtin_modules,
        main,
        visiting: Vec::new(),
        dependencies: BTreeSet::new(),
        module_quota,
        data_limits,
        debug_sink,
        sources,
        semantic_inputs: BTreeMap::new(),
        source_policy,
    };
    loader.load_root(root_module, external_bindings)
}

fn protocol_ref(mut value: crate::ValueRef<'_>) -> crate::ValueRef<'_> {
    while let Some((_, payload)) = value.declared_value_parts() {
        value = payload;
    }
    value
}

fn expect_protocol_record_ref<'a>(
    value: crate::ValueRef<'a>,
    path: &str,
    fields: &[&str],
) -> Result<crate::ValueRef<'a>, ModuleError> {
    let value = protocol_ref(value);
    let actual = value
        .dict_fields()
        .ok_or_else(|| ModuleError::new(format!("{path} must be a record")))?;
    if !actual.iter().copied().eq(fields.iter().copied()) {
        return Err(ModuleError::new(format!(
            "{path} has an invalid field shape"
        )));
    }
    Ok(value)
}

fn protocol_string_ref(value: crate::ValueRef<'_>, path: &str) -> Result<String, ModuleError> {
    protocol_ref(value)
        .as_str()
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| ModuleError::new(format!("{path} must be String")))
}

fn protocol_bool_ref(value: crate::ValueRef<'_>, path: &str) -> Result<bool, ModuleError> {
    match protocol_ref(value).as_atom().as_deref() {
        Some("True") => Ok(true),
        Some("False") => Ok(false),
        _ => Err(ModuleError::new(format!("{path} must be Bool"))),
    }
}

fn protocol_option_string_ref(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<Option<String>, ModuleError> {
    let value = protocol_ref(value);
    if value.as_atom().as_deref() == Some("None") {
        return Ok(None);
    }
    let (tag, payload) = value
        .tagged_parts()
        .ok_or_else(|| ModuleError::new(format!("{path} must be Option(String)")))?;
    if tag.as_atom().as_deref() != Some("Some") {
        return Err(ModuleError::new(format!("{path} must be Option(String)")));
    }
    protocol_string_ref(payload, path).map(Some)
}

fn parse_child_options_ref(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<ChildOptions, ModuleError> {
    let opts = expect_protocol_record_ref(value, path, &["bin", "clear_env", "cwd", "envs"])?;
    let envs = protocol_ref(opts.get("envs").expect("field shape checked"));
    let names = envs
        .dict_fields()
        .ok_or_else(|| ModuleError::new(format!("{path}.envs must be a Dict")))?;
    let envs = names
        .into_iter()
        .map(|name| {
            protocol_option_string_ref(envs.get(name).unwrap(), &format!("{path}.envs.{name}"))
                .map(|value| (name.to_owned(), value))
        })
        .collect::<Result<_, _>>()?;
    Ok(ChildOptions {
        bin: protocol_string_ref(opts.get("bin").unwrap(), &format!("{path}.bin"))?,
        cwd: protocol_option_string_ref(opts.get("cwd").unwrap(), &format!("{path}.cwd"))?,
        envs,
        clear_env: protocol_bool_ref(opts.get("clear_env").unwrap(), &format!("{path}.clear_env"))?,
    })
}

fn parse_stdin_mode_ref(value: crate::ValueRef<'_>) -> Result<ChildStdinMode, ModuleError> {
    match protocol_ref(value).as_atom().as_deref() {
        Some("Piped") => Ok(ChildStdinMode::Piped),
        Some("Inherit") => Ok(ChildStdinMode::Inherit),
        Some("Null") => Ok(ChildStdinMode::Null),
        _ => Err(ModuleError::new("SpawnStdioChild.stdio.stdin is invalid")),
    }
}

fn parse_output_mode_ref(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<ChildOutputMode, ModuleError> {
    match protocol_ref(value).as_atom().as_deref() {
        Some("PipedLine") => Ok(ChildOutputMode::PipedLine),
        Some("PipedToEnd") => Ok(ChildOutputMode::PipedToEnd),
        Some("Inherit") => Ok(ChildOutputMode::Inherit),
        Some("Null") => Ok(ChildOutputMode::Null),
        _ => Err(ModuleError::new(format!("{path} is invalid"))),
    }
}

fn parse_spawn_stdio_child_ref(value: crate::ValueRef<'_>) -> Result<SpawnStdioChild, ModuleError> {
    let child = expect_protocol_record_ref(value, "SpawnStdioChild", &["key", "opts", "stdio"])?;
    let stdio = expect_protocol_record_ref(
        child.get("stdio").unwrap(),
        "SpawnStdioChild.stdio",
        &["stderr", "stdin", "stdout"],
    )?;
    Ok(SpawnStdioChild {
        key: protocol_string_ref(child.get("key").unwrap(), "SpawnStdioChild.key")?,
        opts: parse_child_options_ref(child.get("opts").unwrap(), "SpawnStdioChild.opts")?,
        stdio: ChildStdio {
            stdin: parse_stdin_mode_ref(stdio.get("stdin").unwrap())?,
            stdout: parse_output_mode_ref(
                stdio.get("stdout").unwrap(),
                "SpawnStdioChild.stdio.stdout",
            )?,
            stderr: parse_output_mode_ref(
                stdio.get("stderr").unwrap(),
                "SpawnStdioChild.stdio.stderr",
            )?,
        },
    })
}

fn parse_child_text_ref(value: crate::ValueRef<'_>, path: &str) -> Result<ChildText, ModuleError> {
    let text = expect_protocol_record_ref(value, path, &["data", "key"])?;
    Ok(ChildText {
        key: protocol_string_ref(text.get("key").unwrap(), &format!("{path}.key"))?,
        data: protocol_option_string_ref(text.get("data").unwrap(), &format!("{path}.data"))?,
    })
}

fn semantic_value_json(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<serde_json::Value, ModuleError> {
    let value = protocol_ref(value);
    if let Some(atom) = value.as_atom() {
        return match atom.as_str() {
            "None" => Ok(serde_json::Value::Null),
            "True" => Ok(serde_json::Value::Bool(true)),
            "False" => Ok(serde_json::Value::Bool(false)),
            _ => Err(ModuleError::new(format!(
                "{path} must be a JSON-compatible Value"
            ))),
        };
    }
    let (tag, payload) = value
        .tagged_parts()
        .ok_or_else(|| ModuleError::new(format!("{path} must be Value")))?;
    let tag = tag
        .as_atom()
        .ok_or_else(|| ModuleError::new(format!("{path} has an invalid Value tag")))?;
    match tag.as_str() {
        "Int" => payload
            .as_int()
            .map(serde_json::Value::from)
            .ok_or_else(|| ModuleError::new(format!("{path}.Int must contain Int"))),
        "Float" => payload
            .as_float()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .ok_or_else(|| ModuleError::new(format!("{path}.Float must be finite"))),
        "String" => payload
            .as_str()
            .map(|value| serde_json::Value::String(value.as_str().to_owned()))
            .ok_or_else(|| ModuleError::new(format!("{path}.String must contain String"))),
        "Array" => {
            let length = payload
                .sequence_len()
                .ok_or_else(|| ModuleError::new(format!("{path}.Array must contain Array(Value)")))?;
            (0..length)
                .map(|index| {
                    semantic_value_json(
                        payload.sequence_get(index).expect("index is in range"),
                        &format!("{path}[{index}]"),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(serde_json::Value::Array)
        }
        "Object" => {
            let names = payload.dict_fields().ok_or_else(|| {
                ModuleError::new(format!("{path}.Object must contain Dict(Value)"))
            })?;
            names
                .into_iter()
                .map(|name| {
                    semantic_value_json(
                        payload.get(name).expect("field exists"),
                        &format!("{path}.{name}"),
                    )
                    .map(|value| (name.to_owned(), value))
                })
                .collect::<Result<serde_json::Map<_, _>, _>>()
                .map(serde_json::Value::Object)
        }
        "Bytes" | "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => Err(
            ModuleError::new(format!("{path}.{tag} is not supported by EES JSON calls")),
        ),
        _ => Err(ModuleError::new(format!(
            "{path} has unknown Value tag {tag:?}"
        ))),
    }
}

fn parse_ees_call_ref(value: crate::ValueRef<'_>) -> Result<EesCall, ModuleError> {
    let call = expect_protocol_record_ref(
        value,
        "EesCall",
        &["actor", "input", "key", "operation"],
    )?;
    let key = protocol_string_ref(call.get("key").unwrap(), "EesCall.key")?;
    let actor = protocol_string_ref(call.get("actor").unwrap(), "EesCall.actor")?;
    let operation = protocol_string_ref(call.get("operation").unwrap(), "EesCall.operation")?;
    if key.is_empty() || actor.is_empty() || operation.is_empty() {
        return Err(ModuleError::new(
            "EesCall key, actor, and operation must not be empty",
        ));
    }
    Ok(EesCall {
        key,
        actor,
        operation,
        input: semantic_value_json(call.get("input").unwrap(), "EesCall.input")?,
    })
}

fn runtime_record(heap: &mut Heap, fields: Vec<(&str, Val)>) -> Val {
    let mut fields = fields;
    fields.sort_by(|left, right| left.0.cmp(right.0));
    let names = fields
        .iter()
        .map(|(name, _)| heap.intern(name))
        .collect::<Vec<_>>();
    let values = fields
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let shape = heap.intern_shape(names);
    Val::unknown(DecodedValue::Dict(heap.allocate(Object::Dict {
        shape,
        values: values.into_boxed_slice(),
    })))
}

fn runtime_string(heap: &mut Heap, main: &Heap, value: &str) -> Val {
    Val::unknown(heap.string(Some(main), value))
}

fn runtime_atom(heap: &mut Heap, main: &Heap, value: &str) -> Val {
    Val::unknown(heap.atom(Some(main), value))
}

fn runtime_tagged(heap: &mut Heap, tag: Val, payload: Val) -> Val {
    Val::unknown(DecodedValue::Tagged(
        heap.allocate(Object::Tagged { tag, payload }),
    ))
}

fn runtime_option_string(heap: &mut Heap, main: &Heap, value: Option<String>) -> Val {
    match value {
        None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
        Some(value) => {
            let tag = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Some));
            let payload = runtime_string(heap, main, &value);
            runtime_tagged(heap, tag, payload)
        }
    }
}

fn runtime_child_text(heap: &mut Heap, main: &Heap, text: ChildText) -> Val {
    let key = runtime_string(heap, main, &text.key);
    let data = runtime_option_string(heap, main, text.data);
    runtime_record(heap, vec![("key", key), ("data", data)])
}

fn runtime_json_value(
    heap: &mut Heap,
    main: &Heap,
    type_id: crate::TypeId,
    value: serde_json::Value,
) -> Val {
    let tagged = |heap: &mut Heap, tag: &str, payload| {
        let tag = runtime_atom(heap, main, tag);
        runtime_tagged(heap, tag, payload).with_type_id(type_id)
    };
    match value {
        serde_json::Value::Null => runtime_atom(heap, main, "None").with_type_id(type_id),
        serde_json::Value::Bool(value) => {
            runtime_atom(heap, main, if value { "True" } else { "False" }).with_type_id(type_id)
        }
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                tagged(heap, "Int", Val::unknown(DecodedValue::Int(value)))
            } else {
                tagged(
                    heap,
                    "Float",
                    Val::unknown(DecodedValue::Float(
                        value.as_f64().expect("JSON numbers are finite"),
                    )),
                )
            }
        }
        serde_json::Value::String(value) => {
            let value = runtime_string(heap, main, &value);
            tagged(heap, "String", value)
        }
        serde_json::Value::Array(values) => {
            let values = values
                .into_iter()
                .map(|value| runtime_json_value(heap, main, type_id, value))
                .collect();
            let values = runtime_array(heap, values);
            tagged(heap, "Array", values)
        }
        serde_json::Value::Object(values) => {
            let values = values
                .into_iter()
                .map(|(name, value)| (name, runtime_json_value(heap, main, type_id, value)))
                .collect::<Vec<_>>();
            let fields = values
                .iter()
                .map(|(name, value)| (name.as_str(), *value))
                .collect();
            let values = runtime_record(heap, fields);
            tagged(heap, "Object", values)
        }
    }
}

fn runtime_system_event(
    heap: &mut Heap,
    main: &Heap,
    value_type_id: crate::TypeId,
    event: Option<SystemEvent>,
) -> Result<Val, ModuleError> {
    let Some(event) = event else {
        return Ok(runtime_atom(heap, main, "Initialize"));
    };
    let (tag, payload) = match event {
        SystemEvent::EesReply(reply) => {
            let key = runtime_string(heap, main, &reply.key);
            let (tag, payload) = match reply.result {
                Ok(value) => (
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Ok)),
                    runtime_json_value(heap, main, value_type_id, value),
                ),
                Err(error) => (
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Err)),
                    runtime_string(heap, main, &error),
                ),
            };
            let result = runtime_tagged(heap, tag, payload);
            (
                "EesReply",
                runtime_record(heap, vec![("key", key), ("result", result)]),
            )
        }
        SystemEvent::StdinLine(line) => {
            let line = match line {
                Some(line) => {
                    let line = runtime_string(heap, main, &line);
                    runtime_tagged(
                        heap,
                        Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Some)),
                        line,
                    )
                }
                None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
            };
            ("StdinLine", line)
        }
        SystemEvent::ChildStdout(text) => ("ChildStdout", runtime_child_text(heap, main, text)),
        SystemEvent::ChildStderr(text) => ("ChildStderr", runtime_child_text(heap, main, text)),
        SystemEvent::ChildSpawnResult(result) => {
            let key = runtime_string(heap, main, &result.key);
            let (tag, payload) = match result.result {
                Ok(pid) => (
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Ok)),
                    Val::unknown(DecodedValue::Int(pid)),
                ),
                Err(error) => (
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Err)),
                    runtime_string(heap, main, &error),
                ),
            };
            let result = runtime_tagged(heap, tag, payload);
            (
                "ChildSpawnResult",
                runtime_record(heap, vec![("key", key), ("result", result)]),
            )
        }
        SystemEvent::ChildExited { key, exited } => {
            let key = runtime_string(heap, main, &key);
            let exited = match exited {
                ChildExit::Code(code) => runtime_tagged(
                    heap,
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Ok)),
                    Val::unknown(DecodedValue::Int(code)),
                ),
                ChildExit::Signal(signal) => {
                    let payload = match signal {
                        None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
                        Some(signal) => runtime_tagged(
                            heap,
                            Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Some)),
                            Val::unknown(DecodedValue::Int(signal)),
                        ),
                    };
                    runtime_tagged(
                        heap,
                        Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Err)),
                        payload,
                    )
                }
            };
            (
                "ChildExited",
                runtime_record(heap, vec![("key", key), ("exited", exited)]),
            )
        }
    };
    let tag = runtime_atom(heap, main, tag);
    Ok(runtime_tagged(heap, tag, payload))
}

fn validate_entry_interface(
    interface: &ModuleInterface,
    main_type: &TypeDescriptor,
    state_type: &TypeDescriptor,
    value_type: &TypeDescriptor,
) -> Result<(), ModuleError> {
    let unit_enum = |names: &[&str]| {
        TypeDescriptor::Enum(
            names
                .iter()
                .map(|name| ((*name).to_owned(), None))
                .collect(),
        )
    };
    let bool_type = unit_enum(&["False", "True"]);
    let option_string = TypeDescriptor::Enum(BTreeMap::from([
        ("None".into(), None),
        ("Some".into(), Some(Box::new(TypeDescriptor::String))),
    ]));
    let option_action = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        ("value".into(), TypeDescriptor::Dyn),
    ]));
    let options_type = TypeDescriptor::Array(Box::new(option_action));
    let platform_type = TypeDescriptor::Struct(BTreeMap::from([
        ("arch".into(), TypeDescriptor::String),
        ("os".into(), TypeDescriptor::String),
    ]));
    let data_format = unit_enum(&["Json", "Toml", "Yaml"]);
    let data_source = TypeDescriptor::Struct(BTreeMap::from([
        (
            "default".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(value_type.clone()))),
            ])),
        ),
        ("fmt".into(), data_format),
        ("src".into(), TypeDescriptor::String),
    ]));
    let env_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "args".into(),
            TypeDescriptor::Array(Box::new(TypeDescriptor::String)),
        ),
        (
            "ees".into(),
            TypeDescriptor::Dict(Box::new(TypeDescriptor::String)),
        ),
        ("platform".into(), platform_type),
        (
            "sources".into(),
            TypeDescriptor::Dict(Box::new(data_source.clone())),
        ),
    ]));
    let text_source = TypeDescriptor::Struct(BTreeMap::from([
        ("default".into(), option_string.clone()),
        ("src".into(), TypeDescriptor::String),
    ]));
    let stdin = unit_enum(&["Lined", "Null", "Text"]);
    let caps_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "data_srcs".into(),
            TypeDescriptor::Dict(Box::new(data_source)),
        ),
        (
            "ees".into(),
            TypeDescriptor::Dict(Box::new(TypeDescriptor::String)),
        ),
        ("spawn_child".into(), bool_type.clone()),
        ("stdin".into(), stdin),
        (
            "text_srcs".into(),
            TypeDescriptor::Dict(Box::new(text_source)),
        ),
        (
            "vars".into(),
            TypeDescriptor::Array(Box::new(TypeDescriptor::String)),
        ),
    ]));
    let source_item = |data| {
        TypeDescriptor::Struct(BTreeMap::from([
            ("data".into(), data),
            ("src".into(), TypeDescriptor::String),
        ]))
    };
    let resources_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "data".into(),
            TypeDescriptor::Dict(Box::new(source_item(value_type.clone()))),
        ),
        ("stdin".into(), option_string.clone()),
        (
            "texts".into(),
            TypeDescriptor::Dict(Box::new(source_item(TypeDescriptor::String))),
        ),
        (
            "vars".into(),
            TypeDescriptor::Dict(Box::new(TypeDescriptor::String)),
        ),
    ]));
    let child_text = TypeDescriptor::Struct(BTreeMap::from([
        ("data".into(), option_string.clone()),
        ("key".into(), TypeDescriptor::String),
    ]));
    let child_opts = TypeDescriptor::Struct(BTreeMap::from([
        ("bin".into(), TypeDescriptor::String),
        ("clear_env".into(), unit_enum(&["False", "True"])),
        (
            "cwd".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(TypeDescriptor::String))),
            ])),
        ),
        (
            "envs".into(),
            TypeDescriptor::Dict(Box::new(option_string.clone())),
        ),
    ]));
    let stdin_type = unit_enum(&["Inherit", "Null", "Piped"]);
    let stdout_type = unit_enum(&["Inherit", "Null", "PipedLine", "PipedToEnd"]);
    let stdio_type = TypeDescriptor::Struct(BTreeMap::from([
        ("stderr".into(), stdout_type.clone()),
        ("stdin".into(), stdin_type),
        ("stdout".into(), stdout_type),
    ]));
    let spawn_child = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        ("opts".into(), child_opts.clone()),
        ("stdio".into(), stdio_type),
    ]));
    let child_spawn_result = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        (
            "result".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("Ok".into(), Some(Box::new(TypeDescriptor::Int))),
                ("Err".into(), Some(Box::new(TypeDescriptor::String))),
            ])),
        ),
    ]));
    let child_exited = TypeDescriptor::Struct(BTreeMap::from([
        (
            "exited".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("Ok".into(), Some(Box::new(TypeDescriptor::Int))),
                (
                    "Err".into(),
                    Some(Box::new(TypeDescriptor::Enum(BTreeMap::from([
                        ("None".into(), None),
                        ("Some".into(), Some(Box::new(TypeDescriptor::Int))),
                    ])))),
                ),
            ])),
        ),
        ("key".into(), TypeDescriptor::String),
    ]));
    let ees_reply = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        (
            "result".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("Ok".into(), Some(Box::new(value_type.clone()))),
                ("Err".into(), Some(Box::new(TypeDescriptor::String))),
            ])),
        ),
    ]));
    let event_type = TypeDescriptor::Enum(BTreeMap::from([
        ("Initialize".into(), None),
        ("EesReply".into(), Some(Box::new(ees_reply))),
        ("StdinLine".into(), Some(Box::new(option_string.clone()))),
        ("ChildStdout".into(), Some(Box::new(child_text.clone()))),
        ("ChildStderr".into(), Some(Box::new(child_text.clone()))),
        (
            "ChildSpawnResult".into(),
            Some(Box::new(child_spawn_result)),
        ),
        ("ChildExited".into(), Some(Box::new(child_exited))),
    ]));
    let ees_call = TypeDescriptor::Struct(BTreeMap::from([
        ("actor".into(), TypeDescriptor::String),
        ("input".into(), value_type.clone()),
        ("key".into(), TypeDescriptor::String),
        ("operation".into(), TypeDescriptor::String),
    ]));
    let effect_type = TypeDescriptor::Enum(BTreeMap::from([
        ("EesCall".into(), Some(Box::new(ees_call))),
        ("Exec".into(), Some(Box::new(child_opts))),
        ("Exit".into(), Some(Box::new(TypeDescriptor::Int))),
        ("Output".into(), Some(Box::new(TypeDescriptor::String))),
        ("PostStdin".into(), Some(Box::new(child_text))),
        ("SpawnStdioChild".into(), Some(Box::new(spawn_child))),
    ]));
    let transition_type = TypeDescriptor::Tuple(vec![
        state_type.clone(),
        TypeDescriptor::Array(Box::new(effect_type)),
    ]);
    let reducer_type = TypeDescriptor::Function {
        parameters: vec![state_type.clone(), event_type],
        result: Box::new(transition_type),
    };
    let initializer_type = TypeDescriptor::Function {
        parameters: vec![resources_type, main_type.clone()],
        result: Box::new(TypeDescriptor::Tuple(vec![
            state_type.clone(),
            reducer_type,
        ])),
    };
    let expected = BTreeMap::from([
        (
            "MainType",
            TypeDescriptor::TypeOf(Box::new(main_type.clone())),
        ),
        (
            "State",
            TypeDescriptor::TypeOf(Box::new(state_type.clone())),
        ),
        (
            "config",
            TypeDescriptor::Function {
                parameters: vec![options_type, env_type],
                result: Box::new(TypeDescriptor::Tuple(vec![caps_type, initializer_type])),
            },
        ),
    ]);
    for (name, expected) in expected {
        let scheme = interface
            .exports
            .get(name)
            .ok_or_else(|| ModuleError::new(format!("Entry interface omitted {name}")))?;
        let actual = crate::types::erase_declared_identity(&scheme.body);
        let expected = crate::types::erase_declared_identity(&expected);
        if !scheme.parameters.is_empty()
            || !crate::types::assignable(&actual, &expected)
            || !crate::types::assignable(&expected, &actual)
        {
            return Err(ModuleError::new(format!(
                "Entry.{name} has type {}, expected {}",
                scheme.body.display_name(),
                expected.display_name()
            )));
        }
    }
    Ok(())
}

fn runtime_array(heap: &mut Heap, values: Vec<Val>) -> Val {
    Val::unknown(DecodedValue::Array(
        heap.allocate(Object::Array(values.into_boxed_slice())),
    ))
}

fn runtime_dyn(
    heap: &mut Heap,
    main: &Heap,
    value: Val,
    origin: impl Into<Arc<str>>,
) -> Result<Val, ModuleError> {
    let descriptor = crate::types::infer_value_ref(crate::ValueRef::work(value, heap, main));
    let descriptor_value = heap
        .type_descriptor_value(Some(main), &descriptor)
        .map_err(|error| ModuleError::new(error.to_string()))?;
    Ok(
        value.with_value(DecodedValue::Dyn(heap.allocate(Object::Dyn {
            identity: Arc::new(()),
            descriptor: descriptor_value,
            value,
            scheme: Some(crate::TypeScheme {
                parameters: Vec::new(),
                constraints: Vec::new(),
                body: descriptor,
            }),
            origin: Some(origin.into()),
        }))),
    )
}

fn make_system_options(
    heap: &mut Heap,
    main: &Heap,
    options: &[LoadedOptionAction],
) -> Result<Val, ModuleError> {
    let options = options
        .iter()
        .map(|option| {
            let value = option
                .value
                .relocate_into(heap, main)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            let value = runtime_dyn(heap, main, value, format!("option {:?}", option.key))?;
            let key = runtime_string(heap, main, &option.key);
            Ok(runtime_record(heap, vec![("key", key), ("value", value)]))
        })
        .collect::<Result<Vec<_>, ModuleError>>()?;
    Ok(runtime_array(heap, options))
}

fn make_entry_env(
    heap: &mut Heap,
    main: &Heap,
    arguments: &[String],
    sources: &EntryDataSources,
    ees: &BTreeMap<String, String>,
) -> Val {
    let arguments = arguments
        .iter()
        .map(|argument| runtime_string(heap, main, argument))
        .collect();
    let arguments = runtime_array(heap, arguments);
    let arch = runtime_string(heap, main, std::env::consts::ARCH);
    let os = runtime_string(heap, main, std::env::consts::OS);
    let platform = runtime_record(heap, vec![("arch", arch), ("os", os)]);
    let sources = sources
        .iter()
        .map(|(key, source)| {
            let default = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None));
            let format = runtime_atom(
                heap,
                main,
                match source.format {
                    SystemDataFormat::Json => "Json",
                    SystemDataFormat::Yaml => "Yaml",
                    SystemDataFormat::Toml => "Toml",
                },
            );
            let src = runtime_string(heap, main, &source.src);
            let source = runtime_record(
                heap,
                vec![("default", default), ("fmt", format), ("src", src)],
            );
            (key.clone(), source)
        })
        .collect::<Vec<_>>();
    let sources = allocate_record(heap, sources);
    let ees = ees
        .iter()
        .map(|(name, kind)| (name.as_str(), runtime_string(heap, main, kind)))
        .collect();
    let ees = runtime_record(heap, ees);
    runtime_record(
        heap,
        vec![
            ("args", arguments),
            ("ees", ees),
            ("platform", platform),
            ("sources", sources),
        ],
    )
}

fn parse_system_caps(value: crate::ValueRef<'_>) -> Result<SystemCaps, ModuleError> {
    fn dict<'a>(
        value: crate::ValueRef<'a>,
        path: &str,
    ) -> Result<(crate::ValueRef<'a>, Vec<&'a str>), ModuleError> {
        let value = protocol_ref(value);
        let fields = value
            .dict_fields()
            .ok_or_else(|| ModuleError::new(format!("{path} must be a Dict")))?;
        Ok((value, fields))
    }

    let caps = expect_protocol_record_ref(
        value,
        "Entry.config SystemCaps",
        &["data_srcs", "ees", "spawn_child", "stdin", "text_srcs", "vars"],
    )?;
    let (data, data_keys) = dict(caps.get("data_srcs").unwrap(), "SystemCaps.data_srcs")?;
    let data_sources = data_keys
        .into_iter()
        .map(|key| {
            let path = format!("SystemCaps.data_srcs.{key}");
            let value = expect_protocol_record_ref(
                data.get(key).unwrap(),
                &path,
                &["default", "fmt", "src"],
            )?;
            let format = match protocol_ref(value.get("fmt").unwrap()).as_atom().as_deref() {
                Some("Json") => SystemDataFormat::Json,
                Some("Yaml") => SystemDataFormat::Yaml,
                Some("Toml") => SystemDataFormat::Toml,
                _ => return Err(ModuleError::new(format!("{path}.fmt is invalid"))),
            };
            let src = protocol_string_ref(value.get("src").unwrap(), &format!("{path}.src"))?;
            let default = protocol_ref(value.get("default").unwrap());
            let has_default = if default.as_atom().as_deref() == Some("None") {
                false
            } else {
                let (tag, _) = default.tagged_parts().ok_or_else(|| {
                    ModuleError::new(format!("{path}.default must be Option(Value)"))
                })?;
                if tag.as_atom().as_deref() != Some("Some") {
                    return Err(ModuleError::new(format!(
                        "{path}.default must be Option(Value)"
                    )));
                }
                true
            };
            if key.is_empty() || src.is_empty() {
                return Err(ModuleError::new(format!(
                    "{path} must use non-empty key and src"
                )));
            }
            Ok((
                key.to_owned(),
                SystemDataSource {
                    src,
                    format,
                    has_default,
                },
            ))
        })
        .collect::<Result<_, _>>()?;

    let (ees, ees_keys) = dict(caps.get("ees").unwrap(), "SystemCaps.ees")?;
    let ees = ees_keys
        .into_iter()
        .map(|key| {
            if key.is_empty() {
                return Err(ModuleError::new(
                    "SystemCaps.ees must use non-empty actor names",
                ));
            }
            let kind =
                protocol_string_ref(ees.get(key).unwrap(), &format!("SystemCaps.ees.{key}"))?;
            if kind.is_empty() {
                return Err(ModuleError::new(
                    "SystemCaps.ees must use non-empty component kinds",
                ));
            }
            Ok((key.to_owned(), kind))
        })
        .collect::<Result<_, _>>()?;

    let (texts, text_keys) = dict(caps.get("text_srcs").unwrap(), "SystemCaps.text_srcs")?;
    let text_sources = text_keys
        .into_iter()
        .map(|key| {
            let path = format!("SystemCaps.text_srcs.{key}");
            let value =
                expect_protocol_record_ref(texts.get(key).unwrap(), &path, &["default", "src"])?;
            let src = protocol_string_ref(value.get("src").unwrap(), &format!("{path}.src"))?;
            let default = protocol_option_string_ref(
                value.get("default").unwrap(),
                &format!("{path}.default"),
            )?;
            if key.is_empty() || src.is_empty() {
                return Err(ModuleError::new(
                    "SystemCaps.text_srcs must use non-empty keys and paths",
                ));
            }
            Ok((key.to_owned(), SystemTextSource { src, default }))
        })
        .collect::<Result<_, _>>()?;

    let vars = protocol_ref(caps.get("vars").unwrap());
    let length = vars
        .sequence_len()
        .ok_or_else(|| ModuleError::new("SystemCaps.vars must be Array(String)"))?;
    let mut names = Vec::with_capacity(length);
    let mut unique = BTreeSet::new();
    for index in 0..length {
        let name = protocol_string_ref(
            vars.sequence_get(index).expect("index is in range"),
            &format!("SystemCaps.vars[{index}]"),
        )?;
        if name.is_empty() || !unique.insert(name.clone()) {
            return Err(ModuleError::new(
                "SystemCaps.vars must contain unique non-empty names",
            ));
        }
        names.push(name);
    }
    let stdin = match protocol_ref(caps.get("stdin").unwrap())
        .as_atom()
        .as_deref()
    {
        Some("Text") => SystemStdin::Text,
        Some("Lined") => SystemStdin::Lined,
        Some("Null") => SystemStdin::Null,
        _ => return Err(ModuleError::new("SystemCaps.stdin is invalid")),
    };
    let spawn_child =
        protocol_bool_ref(caps.get("spawn_child").unwrap(), "SystemCaps.spawn_child")?;
    Ok(SystemCaps {
        data_sources,
        ees,
        spawn_child,
        text_sources,
        vars: names,
        stdin,
    })
}

fn concrete_module_descriptor(interface: &ModuleInterface) -> Result<TypeDescriptor, ModuleError> {
    let mut fields = BTreeMap::new();
    for (name, scheme) in &interface.exports {
        if !scheme.parameters.is_empty() {
            return Err(ModuleError::new(format!(
                "Main export {name:?} is generic and has no concrete Entry boundary type"
            )));
        }
        fields.insert(name.clone(), scheme.body.clone());
    }
    Ok(TypeDescriptor::Struct(fields))
}
