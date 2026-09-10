#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub module_quota: Quota,
    pub session_quota: Quota,
    pub data_limits: DataLimits,
}


struct NoProcessRunHost;

fn empty_system_resources(
    context: &mut crate::CallContext<'_, '_>,
) -> Result<(), crate::NativeError> {
    let data = context.scratch()?;
    let texts = context.scratch()?;
    let vars = context.scratch()?;
    let stdin = context.scratch()?;
    context.make_dict(data, &[])?;
    context.make_dict(texts, &[])?;
    context.make_dict(vars, &[])?;
    context.set_none(stdin)?;
    context.make_dict(
        context.result(),
        &[
            ("data".into(), data),
            ("texts".into(), texts),
            ("vars".into(), vars),
            ("stdin".into(), stdin),
        ],
    )
}

impl RunHost for NoProcessRunHost {
    fn resources_provider(&mut self) -> crate::NativeFunction {
        crate::NativeFunction::new(
            "host.prepare_system_resources.empty",
            3,
            empty_system_resources,
        )
    }

    fn ees_actors(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !caps.data_sources.is_empty()
                || !caps.ees.is_empty()
                || !caps.text_sources.is_empty()
                || !caps.vars.is_empty()
                || caps.stdin != SystemStdin::Null
            {
                return Err("this Host does not provide initialization capabilities".into());
            }
            Ok(())
        })
    }

    fn read_data_source(
        &mut self,
        _source: &SystemDataSource,
        _max_bytes: usize,
    ) -> RunHostFuture<'_, Result<Option<String>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn ees_call(&mut self, _call: EesCall) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Err("this Host does not provide EES actors".into()) })
    }

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}
