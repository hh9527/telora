//! A continuous Service build binds preparation to its own compiler evidence.
//! Constructors and session ownership prevent mixing another build's A.result.
pub use crate::specialization::Report;
use crate::{
    artifact::Manifest,
    session::Session,
    specialization::{RuntimeDependencies, RuntimePlan},
    transform_service::{Initialization, SourceReader, TransformSession},
};
use telora_core::{SourceDatabase, SourceId, data_plan::Format, mir::SealedExecutable};

#[derive(Clone, Copy)]
pub enum SnapshotMode {
    Runtime,
    Full,
}

pub struct ServiceBuild {
    bytes: Vec<u8>,
    plan: RuntimePlan,
}

pub struct LoadedService<'build> {
    build: &'build ServiceBuild,
    session: Session,
}

pub struct ServicePreparation<'build> {
    build: &'build ServiceBuild,
    service: TransformSession,
}

pub struct PreparedService<'build> {
    build: &'build ServiceBuild,
    snapshot: telora_wasm_shared::snapshot_artifact::Snapshot,
    dependencies: Option<RuntimeDependencies>,
    metadata: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ServiceBuild {
    pub fn compile(executable: &SealedExecutable<'_>) -> Result<Self, String> {
        let linked = crate::codegen::compile_service_build(executable)?;
        Ok(Self {
            bytes: linked.bytes,
            plan: linked.runtime_plan,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn manifest(&self) -> Result<Manifest, String> {
        Manifest::read(&self.bytes)
    }

    pub fn bundle_data(
        mut self,
        sources: &SourceDatabase,
        plans: &[(u32, SourceId, Format)],
        fuel: u64,
        memory: usize,
    ) -> Result<Self, String> {
        self.bytes = crate::bundle::build_with_limits(&self.bytes, sources, plans, fuel, memory)?;
        Ok(self)
    }

    pub fn load(
        &self,
        initialization_fuel: u64,
        memory: usize,
        request_fuel: u64,
        observer: &mut dyn FnMut(&'static str, std::time::Duration),
    ) -> Result<LoadedService<'_>, String> {
        let mut session =
            Session::load_with_limits_observed(&self.bytes, initialization_fuel, memory, observer)?;
        session.set_request_fuel(request_fuel);
        Ok(LoadedService {
            build: self,
            session,
        })
    }
}

impl<'build> LoadedService<'build> {
    pub fn prepare(self) -> Result<ServicePreparation<'build>, String> {
        Ok(ServicePreparation {
            build: self.build,
            service: TransformSession::new(self.session)?,
        })
    }
}

impl<'build> ServicePreparation<'build> {
    pub fn sources(&self) -> &[String] {
        self.service.sources()
    }

    pub fn initialize_readers<'a>(
        &mut self,
        sources: impl IntoIterator<Item = Result<SourceReader<'a>, String>>,
        limit: usize,
    ) -> Result<Initialization, String> {
        self.service.initialize_readers(sources, limit)
    }

    pub fn seal(mut self, mode: SnapshotMode) -> Result<PreparedService<'build>, String> {
        let metadata = self.service.take_initialization_metadata()?;
        let dependencies = match mode {
            SnapshotMode::Runtime => Some(self.service.seal_runtime_initialization()?),
            SnapshotMode::Full => {
                self.service.seal_initialization()?;
                None
            }
        };
        Ok(PreparedService {
            build: self.build,
            snapshot: self.service.publication_snapshot()?,
            dependencies,
            metadata,
        })
    }
}

impl PreparedService<'_> {
    pub fn publish(self) -> Result<(Vec<u8>, Option<Report>), String> {
        self.publish_observed(&mut |_, _| {})
    }

    pub fn publish_observed(
        self,
        observer: &mut dyn FnMut(&'static str, std::time::Duration),
    ) -> Result<(Vec<u8>, Option<Report>), String> {
        let (bytes, report) = if let Some(dependencies) = self.dependencies {
            let started = std::time::Instant::now();
            let specialized =
                crate::specialization::runtime(&self.build.bytes, &self.build.plan, &dependencies);
            observer("runtime_specialization", started.elapsed());
            let (bytes, mut report) = specialized?;
            report.snapshot_bytes = self.snapshot.guest.len();
            (bytes, Some(report))
        } else {
            (self.build.bytes.clone(), None)
        };
        let bytes = crate::publication::attach_snapshot(&bytes, &self.snapshot)?;
        Ok((
            crate::publication::attach_metadata(&bytes, &self.metadata)?,
            report,
        ))
    }
}
