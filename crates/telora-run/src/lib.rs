//! Reference artifact runner. No compiler, MIR, codegen, or package dependencies.
mod artifact;
mod backend;
mod engine;
mod fuel_quota;
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
    pub initialization_fuel: Option<u64>,
    pub request_fuel: Option<u64>,
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
    artifact_snapshot: Option<telora_wasm_shared::snapshot_artifact::Snapshot>,
    baseline: Option<Baseline>,
    initialization_fuel: u64,
    request_fuel: u64,
    initialization_quota: Option<fuel_quota::FuelQuota>,
    request_fuel_consumed: u64,
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
        let initialization_fuel = fuel_override(
            artifact.publication.initialization_fuel,
            options.initialization_fuel,
        )?;
        let request_fuel = fuel_override(artifact.publication.request_fuel, options.request_fuel)?;
        let memory_limit = options
            .memory_limit
            .unwrap_or(usize::try_from(artifact.publication.memory_limit)?);
        ensure!(memory_limit > 0, "execution limits must be positive");
        let load_limit = usize::try_from(artifact.publication.memory_limit)?;
        let now = Instant::now();
        let module = backend::compile(bytes)?;
        let module_ms = now.elapsed().as_secs_f64() * 1000.;
        let now = Instant::now();
        // CLI overrides constrain requests; ordinary initialization retains its build budget.
        let guest = Guest::instantiate(module, initialization_fuel, load_limit)?;
        let instance_ms = now.elapsed().as_secs_f64() * 1000.;
        Ok(Self {
            guest,
            publication: artifact.publication,
            modules: artifact.modules,
            artifact_snapshot: artifact.snapshot,
            baseline: None,
            timings: Timings {
                metadata_ms,
                module_ms,
                instance_ms,
                ..Default::default()
            },
            initialization_fuel,
            request_fuel,
            initialization_quota: None,
            request_fuel_consumed: 0,
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
        let metered = if phase == "instance-compacted" {
            self.phases.last().map_or(0, |usage| usage.fuel_consumed)
        } else {
            self.initialization_fuel
                .saturating_sub(self.guest.store.get_fuel()?)
        };
        self.phases.push(PhaseUsage {
            phase,
            elapsed_ms: self
                .initialization_started
                .map_or(0., |start| start.elapsed().as_secs_f64() * 1000.),
            linear_memory_bytes: self.guest.memory.data_size(&self.guest.store),
            language_heap_bytes,
            fuel_consumed: metered,
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
        self.initialization_quota = Some(fuel_quota::FuelQuota::new(self.initialization_fuel));
        self.record_phase("instantiated")?;
        if sources.is_empty()
            && let Some(snapshot) = self.artifact_snapshot.take()
        {
            self.guest.import_snapshot(&snapshot.guest)?;
            let globals = restore_artifact_globals(&mut self.guest, snapshot.globals)?;
            self.modules.clear();
            self.baseline = Some(Baseline {
                snapshot: snapshot.guest,
                memory_bytes: self.guest.memory.data_size(&self.guest.store),
                globals,
            });
            self.record_phase("snapshot-imported")?;
            self.ready = true;
            self.poisoned = false;
            self.reset()?;
            self.timings.initialize_ms = now.elapsed().as_secs_f64() * 1000.;
            self.initialization_quota = None;
            return Ok(vec![]);
        }
        self.artifact_snapshot = None;
        for module in &self.modules {
            self.guest
                .inject_module(module, self.initialization_quota.as_mut().unwrap())?;
        }
        self.record_phase("bundled-modules-injected")?;
        let names = self
            .guest
            .sources(self.initialization_quota.as_mut().unwrap())?;
        self.record_phase("binary-closed")?;
        self.guest.inject_named_sources(
            &names,
            sources,
            self.initialization_quota.as_mut().unwrap(),
        )?;
        self.record_phase("sources-injected")?;
        let status = self.initialization_quota.as_mut().unwrap().call(
            &mut self.guest.store,
            self.guest.exports.create,
            (),
        )?;
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
            self.initialization_fuel,
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
        self.initialization_quota = None;
        Ok(diagnostics)
    }

    pub fn request(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        ensure!(self.ready, "service needs initialization");
        let reset_start = Instant::now();
        self.reset()?;
        self.timings.reset_ms = reset_start.elapsed().as_secs_f64() * 1000.;
        self.poisoned = true;
        self.guest.store.set_fuel(self.request_fuel)?;
        let now = Instant::now();
        let mut quota = fuel_quota::FuelQuota::new(self.request_fuel);
        let result = self.guest.request_with_quota(input, &mut quota);
        self.request_fuel_consumed = quota.consumed();
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
        self.guest.store.set_fuel(self.initialization_fuel)?;
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
                Guest::instantiate(self.guest.module.clone(), self.initialization_fuel, limit)?;
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
        self.guest.store.set_fuel(self.request_fuel)?;
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
            fuel_limit: self.request_fuel,
            fuel_consumed: self.request_fuel_consumed,
            memory_bytes: self.guest.memory.data_size(&self.guest.store),
            memory_limit_bytes: self
                .memory_limit
                .saturating_add(self.baseline.as_ref().map_or(0, |b| b.memory_bytes)),
            initialization,
            phases: std::mem::take(&mut self.phases),
        }
    }
}

fn restore_artifact_globals(
    guest: &mut Guest,
    globals: Vec<(String, telora_wasm_shared::snapshot_artifact::GlobalValue)>,
) -> Result<Vec<(String, backend::runtime::Val)>> {
    use telora_wasm_shared::snapshot_artifact::GlobalValue;
    let mut expected = backend::globals(guest.instance, &mut guest.store)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    let mut actual = globals
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    expected.sort();
    actual.sort();
    ensure!(
        actual == expected && !actual.windows(2).any(|names| names[0] == names[1]),
        "snapshot globals do not match the module"
    );
    let globals = globals
        .into_iter()
        .map(|(name, value)| {
            let value = match value {
                GlobalValue::I32(value) => backend::runtime::Val::I32(value),
                GlobalValue::I64(value) => backend::runtime::Val::I64(value),
                GlobalValue::F32(value) => {
                    backend::runtime::Val::F32(backend::runtime::F32::from_bits(value))
                }
                GlobalValue::F64(value) => {
                    backend::runtime::Val::F64(backend::runtime::F64::from_bits(value))
                }
            };
            (name, value)
        })
        .collect::<Vec<_>>();
    for (name, value) in &globals {
        guest
            .instance
            .get_global(&guest.store, name)
            .ok_or_else(|| anyhow::anyhow!("missing snapshot global {name:?}"))?
            .set(&mut guest.store, value.clone())?;
    }
    Ok(globals)
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

fn fuel_override(published: u64, millions: Option<u64>) -> Result<u64> {
    millions.map_or(Ok(published), |n| {
        n.checked_mul(1_000_000)
            .filter(|&fuel| fuel != 0)
            .ok_or_else(|| anyhow::anyhow!("fuel budget must be positive and fit u64"))
    })
}

#[cfg(test)]
mod fuel_override_tests {
    #[test]
    fn overrides_published_fuel_in_millions() {
        assert_eq!(
            super::fuel_override(5_000_000_000, None).unwrap(),
            5_000_000_000
        );
        assert_eq!(
            super::fuel_override(5_000_000_000, Some(2500)).unwrap(),
            2_500_000_000
        );
        assert!(super::fuel_override(1, Some(0)).is_err());
        assert!(super::fuel_override(1, Some(u64::MAX)).is_err());
    }
}
