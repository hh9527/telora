//! Build-time engine choice, runtime strategy choice within Wasmtime.
use anyhow::Result;

#[cfg(not(feature = "wasmtime"))]
pub(crate) use wasmi as runtime;
#[cfg(feature = "wasmtime")]
pub(crate) use wasmtime as runtime;

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[cfg(not(feature = "wasmtime"))]
    #[default]
    Wasmi,
    /// Cranelift lowers to Pulley bytecode, which is interpreted (not native JIT).
    #[cfg(feature = "wasmtime")]
    Pulley,
    /// Speed-optimized Pulley bytecode, still interpreted rather than native code.
    #[cfg(feature = "wasmtime")]
    PulleySpeed,
    /// Wasmtime's low-latency baseline native compiler.
    #[cfg(feature = "wasmtime")]
    Winch,
    #[cfg(feature = "wasmtime")]
    CraneliftNone,
    #[cfg(feature = "wasmtime")]
    #[default]
    CraneliftSpeed,
    #[cfg(feature = "wasmtime")]
    CraneliftSpeedAndSize,
}

#[cfg(not(feature = "wasmtime"))]
pub(crate) fn compile(bytes: &[u8], _: Mode) -> Result<runtime::Module> {
    let mut config = runtime::Config::default();
    config.consume_fuel(true);
    Ok(runtime::Module::new(&runtime::Engine::new(&config), bytes)?)
}

#[cfg(feature = "wasmtime")]
pub(crate) fn compile(bytes: &[u8], mode: Mode) -> Result<runtime::Module> {
    use runtime::{Config, Engine, Module, OptLevel, Strategy};
    if matches!(mode, Mode::Winch) {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let wasmparser::Payload::CodeSectionEntry(body) = payload? {
                let mut ops = body.get_operators_reader()?;
                while !ops.eof() {
                    if matches!(
                        ops.read()?,
                        wasmparser::Operator::ReturnCall { .. }
                            | wasmparser::Operator::ReturnCallIndirect { .. }
                            | wasmparser::Operator::ReturnCallRef { .. }
                    ) {
                        anyhow::bail!(
                            "Winch does not support this artifact's tail calls; use --mode cranelift-none for unoptimized native execution"
                        );
                    }
                }
            }
        }
    }
    let mut config = Config::new();
    config.consume_fuel(true);
    match mode {
        Mode::Pulley | Mode::PulleySpeed => {
            let target = match (
                cfg!(target_pointer_width = "64"),
                cfg!(target_endian = "little"),
            ) {
                (true, true) => "pulley64",
                (true, false) => "pulley64be",
                (false, true) => "pulley32",
                (false, false) => "pulley32be",
            };
            config.target(target)?;
            config.strategy(Strategy::Cranelift).cranelift_opt_level(
                if matches!(mode, Mode::PulleySpeed) {
                    OptLevel::Speed
                } else {
                    OptLevel::None
                },
            );
        }
        Mode::Winch => {
            config.strategy(Strategy::Winch);
        }
        Mode::CraneliftNone => {
            config
                .strategy(Strategy::Cranelift)
                .cranelift_opt_level(OptLevel::None);
        }
        Mode::CraneliftSpeed => {
            config
                .strategy(Strategy::Cranelift)
                .cranelift_opt_level(OptLevel::Speed);
        }
        Mode::CraneliftSpeedAndSize => {
            config
                .strategy(Strategy::Cranelift)
                .cranelift_opt_level(OptLevel::SpeedAndSize);
        }
    }
    // No persistent compilation cache: measurements start from the same Wasm bytes.
    Ok(Module::new(&Engine::new(&config)?, bytes)?)
}

pub(crate) fn instantiate(
    module: &runtime::Module,
    store: &mut runtime::Store<runtime::StoreLimits>,
) -> Result<runtime::Instance> {
    let linker = runtime::Linker::new(module.engine());
    #[cfg(not(feature = "wasmtime"))]
    {
        Ok(linker.instantiate_and_start(store, module)?)
    }
    #[cfg(feature = "wasmtime")]
    {
        Ok(linker.instantiate(store, module)?)
    }
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
            #[cfg(not(feature = "wasmtime"))]
            let mutable = global.ty(&*store).mutability().is_mut();
            #[cfg(feature = "wasmtime")]
            let mutable = global.ty(&*store).mutability() == runtime::Mutability::Var;
            mutable.then(|| (name, global.get(&mut *store)))
        })
        .collect()
}
