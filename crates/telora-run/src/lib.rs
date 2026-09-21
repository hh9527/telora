//! Reference artifact runner. No compiler, MIR, codegen, or package dependencies.
mod artifact;
mod backend;
mod engine;
mod input;
pub mod transport;
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
    pub initialization: [u32; 5],
    pub phases: Vec<PhaseUsage>,
}

#[derive(serde::Serialize)]
pub struct PhaseUsage {
    pub phase: &'static str,
    pub elapsed_ms: f64,
    pub linear_memory_bytes: usize,
    pub language_heap_bytes: u32,
    pub fuel_consumed: u64,
    pub rss_bytes: Option<u64>,
}

struct Baseline {
    snapshot: Vec<u8>,
    memory_bytes: usize,
    globals: Vec<(String, backend::runtime::Val)>,
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
    initialization_started: Option<Instant>,
    phases: Vec<PhaseUsage>,
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
        let load_limit = usize::try_from(artifact.publication.memory_limit)?;
        let now = Instant::now();
        let module = backend::compile(bytes)?;
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
            initialization_started: None,
            phases: vec![],
        })
    }

    fn record_phase(&mut self, phase: &'static str) -> Result<()> {
        let language_heap_bytes = self
            .guest
            .exports
            .heap_bytes
            .call(&mut self.guest.store, ())?;
        let fuel_remaining = self.guest.store.get_fuel()?;
        self.phases.push(PhaseUsage {
            phase,
            elapsed_ms: self
                .initialization_started
                .map_or(0., |start| start.elapsed().as_secs_f64() * 1000.),
            linear_memory_bytes: self.guest.memory.data_size(&self.guest.store),
            language_heap_bytes,
            fuel_consumed: self.publication.fuel.saturating_sub(fuel_remaining),
            rss_bytes: current_rss_bytes(),
        });
        Ok(())
    }

    /// Call once before requests. Failure never publishes a usable service.
    pub fn initialize(&mut self, sources: &[SourceInput]) -> Result<Vec<serde_json::Value>> {
        ensure!(
            !self.ready && !self.poisoned,
            "initialization cannot be repeated"
        );
        self.poisoned = true;
        let now = Instant::now();
        self.initialization_started = Some(now);
        self.record_phase("instantiated")?;
        for module in &self.modules {
            self.guest.inject_module(module)?;
        }
        self.record_phase("bundled-modules-injected")?;
        let names = self.guest.sources()?;
        self.record_phase("binary-closed")?;
        self.guest.inject_named_sources(&names, sources)?;
        self.record_phase("sources-injected")?;
        let status = self.guest.exports.create.call(&mut self.guest.store, ())?;
        self.record_phase("service-created")?;
        let diagnostics = self.guest.diagnostics()?;
        if status != 0 {
            return Err(InitializationError { diagnostics }.into());
        }
        self.modules.clear();
        let globals = backend::globals(self.guest.instance, &mut self.guest.store);
        let snapshot = self.guest.export_snapshot()?;
        let mut compact = Guest::instantiate(
            self.guest.module.clone(),
            self.publication.fuel,
            usize::try_from(self.publication.memory_limit)?,
        )?;
        compact.import_snapshot(&snapshot)?;
        for (name, value) in &globals {
            compact
                .instance
                .get_global(&mut compact.store, name)
                .ok_or_else(|| anyhow::anyhow!("missing reset global"))?
                .set(&mut compact.store, value.clone())?;
        }
        self.guest = compact;
        self.baseline = Some(Baseline {
            snapshot,
            memory_bytes: self.guest.memory.data_size(&self.guest.store),
            globals,
        });
        self.record_phase("instance-compacted")?;
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
            .memory_bytes
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
            guest.import_snapshot(&baseline.snapshot)?;
            self.guest = guest;
        }
        for (name, value) in &baseline.globals {
            self.guest
                .instance
                .get_global(&mut self.guest.store, name)
                .ok_or_else(|| anyhow::anyhow!("missing reset global"))?
                .set(&mut self.guest.store, value.clone())?;
        }
        *self.guest.store.data_mut() = engine::limits(limit);
        self.guest.store.set_fuel(self.fuel)?;
        self.poisoned = false;
        Ok(())
    }

    pub fn usage(&mut self) -> Usage {
        let mut initialization = [0; 5];
        for (index, value) in initialization.iter_mut().enumerate() {
            *value = self
                .guest
                .exports
                .initialization_stat
                .call(&mut self.guest.store, index as u32)
                .unwrap_or(0);
        }
        Usage {
            fuel_limit: self.fuel,
            fuel_consumed: self
                .fuel
                .saturating_sub(self.guest.store.get_fuel().unwrap_or(0)),
            memory_bytes: self.guest.memory.data_size(&self.guest.store),
            memory_limit_bytes: self
                .memory_limit
                .saturating_add(self.baseline.as_ref().map_or(0, |b| b.memory_bytes)),
            initialization,
            phases: std::mem::take(&mut self.phases),
        }
    }
}

fn current_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_ascii_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}
