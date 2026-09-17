//! Reference artifact runner. No compiler, MIR, codegen, or package dependencies.
#[cfg(not(feature = "wasmi"))]
compile_error!("select the wasmi feature (the first implemented backend)");
mod artifact;
#[cfg(feature = "wasmi")]
mod engine;
mod input;
use anyhow::{Result, ensure};
pub use artifact::Publication;

#[derive(Debug)]
pub struct InitializationError {
    pub diagnostics: Vec<serde_json::Value>,
}

impl std::fmt::Display for InitializationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("service initialization failed")
    }
}

impl std::error::Error for InitializationError {}
use engine::Guest;
pub use input::SourceInput;
use std::time::Instant;

#[derive(Clone, Copy, Default)]
pub struct Options {
    pub fuel: Option<u64>,
    pub memory_limit: Option<usize>,
}

#[derive(Default, serde::Serialize)]
pub struct Timings {
    pub metadata_ms: f64,
    pub module_ms: f64,
    pub instance_ms: f64,
    pub initialize_ms: f64,
    pub reset_ms: f64,
    pub request_ms: f64,
}

#[derive(serde::Serialize)]
pub struct Usage {
    pub fuel_limit: u64,
    pub fuel_consumed: u64,
    pub memory_bytes: usize,
    pub memory_limit_bytes: usize,
}

struct Baseline {
    memory: Vec<u8>,
    globals: Vec<(String, wasmi::Val)>,
}

pub struct Runner {
    guest: Guest,
    pub publication: Publication,
    pub timings: Timings,
    modules: Vec<artifact::ModuleData>,
    baseline: Option<Baseline>,
    fuel: u64,
    memory_limit: usize,
    ready: bool,
    poisoned: bool,
}

impl Runner {
    pub fn load(bytes: &[u8], options: Options) -> Result<Self> {
        let now = Instant::now();
        let artifact = artifact::Artifact::read(bytes)?;
        let metadata_ms = now.elapsed().as_secs_f64() * 1000.;
        let fuel = options.fuel.unwrap_or(artifact.publication.fuel);
        let memory_limit = options
            .memory_limit
            .unwrap_or(usize::try_from(artifact.publication.memory_limit)?);
        ensure!(
            fuel > 0 && memory_limit > 0,
            "execution limits must be positive"
        );
        let load_limit = if artifact.publication.initialized {
            artifact
                .memory_bytes
                .checked_add(memory_limit)
                .ok_or_else(|| anyhow::anyhow!("memory limit overflow"))?
        } else {
            usize::try_from(artifact.publication.memory_limit)?
        };
        let now = Instant::now();
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, bytes)?;
        let module_ms = now.elapsed().as_secs_f64() * 1000.;
        let now = Instant::now();
        // CLI overrides constrain requests; ordinary initialization retains its build budget.
        let guest = Guest::instantiate(module, artifact.publication.fuel, load_limit)?;
        let instance_ms = now.elapsed().as_secs_f64() * 1000.;
        Ok(Self {
            guest,
            publication: artifact.publication,
            modules: artifact.modules,
            baseline: None,
            timings: Timings {
                metadata_ms,
                module_ms,
                instance_ms,
                ..Default::default()
            },
            fuel,
            memory_limit,
            ready: false,
            poisoned: false,
        })
    }

    /// Snapshot artifacts skip initialization; they reject replacement sources.
    /// Call once before requests. Failure never publishes a usable service.
    pub fn initialize(&mut self, sources: &[SourceInput]) -> Result<Vec<serde_json::Value>> {
        ensure!(
            !self.ready && !self.poisoned,
            "initialization cannot be repeated"
        );
        self.poisoned = true;
        let now = Instant::now();
        let mut diagnostics = vec![];
        if self.publication.initialized {
            ensure!(
                sources.is_empty(),
                "snapshot sources are already fixed; rebuild to replace them"
            );
        } else {
            for module in &self.modules {
                self.guest.inject_module(module)?;
            }
            self.guest.inject_sources(sources)?;
            let status = self.guest.exports.create.call(&mut self.guest.store, ())?;
            diagnostics = self.guest.diagnostics()?;
            if status != 0 {
                return Err(InitializationError { diagnostics }.into());
            }
        }
        self.modules.clear();
        let globals = self
            .guest
            .instance
            .exports(&self.guest.store)
            .filter_map(|export| {
                let name = export.name().to_owned();
                if !name.starts_with("telora_reset_global_") {
                    return None;
                }
                let global = export.into_global()?;
                global
                    .ty(&self.guest.store)
                    .mutability()
                    .is_mut()
                    .then(|| (name, global.get(&self.guest.store)))
            })
            .collect();
        self.baseline = Some(Baseline {
            memory: self.guest.memory.data(&self.guest.store).to_vec(),
            globals,
        });
        self.ready = true;
        self.poisoned = false;
        self.reset()?;
        self.timings.initialize_ms = now.elapsed().as_secs_f64() * 1000.;
        Ok(diagnostics)
    }

    pub fn request(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        ensure!(self.ready, "service needs initialization");
        let reset_start = Instant::now();
        self.reset()?;
        self.timings.reset_ms = reset_start.elapsed().as_secs_f64() * 1000.;
        self.poisoned = true;
        self.guest.store.set_fuel(self.fuel)?;
        let now = Instant::now();
        let result = self.guest.request(input);
        self.timings.request_ms = now.elapsed().as_secs_f64() * 1000.;
        if result.is_ok() {
            self.poisoned = false;
        }
        result
    }

    pub fn reset(&mut self) -> Result<()> {
        let baseline = self
            .baseline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("service is not initialized"))?;
        let limit = baseline
            .memory
            .len()
            .checked_add(self.memory_limit)
            .ok_or_else(|| anyhow::anyhow!("memory limit overflow"))?;
        // Cleanup/bootstrap is outside the next request's quota.
        self.guest.store.set_fuel(self.publication.fuel)?;
        if self.poisoned
            || self
                .guest
                .exports
                .reset
                .call(&mut self.guest.store, ())
                .is_err()
        {
            self.poisoned = true;
            let mut guest =
                Guest::instantiate(self.guest.module.clone(), self.publication.fuel, limit)?;
            let current = guest.memory.data_size(&guest.store);
            if baseline.memory.len() > current {
                guest.memory.grow(
                    &mut guest.store,
                    ((baseline.memory.len() - current) / 65536) as u64,
                )?;
            }
            guest.memory.write(&mut guest.store, 0, &baseline.memory)?;
            self.guest = guest;
        }
        for (name, value) in &baseline.globals {
            self.guest
                .instance
                .get_global(&self.guest.store, name)
                .ok_or_else(|| anyhow::anyhow!("missing reset global"))?
                .set(&mut self.guest.store, value.clone())?;
        }
        *self.guest.store.data_mut() = engine::limits(limit);
        self.guest.store.set_fuel(self.fuel)?;
        self.poisoned = false;
        Ok(())
    }

    pub fn usage(&self) -> Usage {
        Usage {
            fuel_limit: self.fuel,
            fuel_consumed: self
                .fuel
                .saturating_sub(self.guest.store.get_fuel().unwrap_or(0)),
            memory_bytes: self.guest.memory.data_size(&self.guest.store),
            memory_limit_bytes: self
                .memory_limit
                .saturating_add(self.baseline.as_ref().map_or(0, |b| b.memory.len())),
        }
    }
}
