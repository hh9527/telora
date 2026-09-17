use anyhow::Result;

pub(crate) use wasmi as runtime;

pub(crate) fn compile(bytes: &[u8]) -> Result<runtime::Module> {
    let mut config = runtime::Config::default();
    config.consume_fuel(true);
    Ok(runtime::Module::new(&runtime::Engine::new(&config), bytes)?)
}

pub(crate) fn instantiate(
    module: &runtime::Module,
    store: &mut runtime::Store<runtime::StoreLimits>,
) -> Result<runtime::Instance> {
    let linker = runtime::Linker::new(module.engine());
    Ok(linker.instantiate_and_start(store, module)?)
}

pub(crate) fn globals(
    instance: runtime::Instance,
    store: &mut runtime::Store<runtime::StoreLimits>,
) -> Vec<(String, runtime::Val)> {
    let globals: Vec<_> = instance
        .exports(&mut *store)
        .filter_map(|export| {
            let name = export.name().to_owned();
            if !name.starts_with("telora_reset_global_") {
                return None;
            }
            Some((name, export.into_global()?))
        })
        .collect();
    globals
        .into_iter()
        .filter_map(|(name, global)| {
            let mutable = global.ty(&*store).mutability().is_mut();
            mutable.then(|| (name, global.get(&mut *store)))
        })
        .collect()
}
