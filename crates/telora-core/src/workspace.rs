use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::document::{DocumentError, DocumentSnapshot, DocumentVersion, TextEdit};
use crate::module::{Engine, ModuleError};
use crate::query::{CancellationToken, QueryContext, QueryError, Revision, RevisionClock};
use crate::semantic::WorkspaceSnapshot;

pub struct Workspace {
    root: PathBuf,
    engine: Engine,
    clock: RevisionClock,
    state: RwLock<WorkspaceState>,
}

#[derive(Default)]
struct WorkspaceState {
    overlays: BTreeMap<PathBuf, DocumentSnapshot>,
    published: Option<Arc<WorkspaceSnapshot>>,
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>, engine: Engine) -> Result<Self, WorkspaceError> {
        Ok(Self {
            root: canonical_document_path(root.as_ref())?,
            engine,
            clock: RevisionClock::default(),
            state: RwLock::new(WorkspaceState::default()),
        })
    }

    pub fn revision(&self) -> Revision {
        self.clock.current()
    }

    pub fn context(&self) -> QueryContext {
        QueryContext::current(self.clock.clone())
    }

    pub fn cancellable_context(&self, cancellation: CancellationToken) -> QueryContext {
        QueryContext::new(self.revision(), self.clock.clone(), cancellation)
    }

    pub fn open(
        &self,
        path: impl AsRef<Path>,
        version: DocumentVersion,
        text: impl AsRef<str>,
    ) -> Result<Revision, WorkspaceError> {
        let path = canonical_document_path(path.as_ref())?;
        let mut state = self.state.write().expect("workspace state poisoned");
        state
            .overlays
            .insert(path, DocumentSnapshot::new(version, text));
        Ok(self.clock.advance())
    }

    pub fn change(
        &self,
        path: impl AsRef<Path>,
        expected: DocumentVersion,
        version: DocumentVersion,
        edits: &[TextEdit],
    ) -> Result<Revision, WorkspaceError> {
        let path = canonical_document_path(path.as_ref())?;
        let mut state = self.state.write().expect("workspace state poisoned");
        let current = state
            .overlays
            .get(&path)
            .ok_or_else(|| WorkspaceError::DocumentNotOpen(path.clone()))?;
        let changed = current.changed(expected, version, edits)?;
        state.overlays.insert(path, changed);
        Ok(self.clock.advance())
    }

    pub fn close(&self, path: impl AsRef<Path>) -> Result<Revision, WorkspaceError> {
        let path = canonical_document_path(path.as_ref())?;
        let mut state = self.state.write().expect("workspace state poisoned");
        if state.overlays.remove(&path).is_none() {
            return Err(WorkspaceError::DocumentNotOpen(path));
        }
        Ok(self.clock.advance())
    }

    pub fn document(&self, path: impl AsRef<Path>) -> Result<DocumentSnapshot, WorkspaceError> {
        let path = canonical_document_path(path.as_ref())?;
        self.state
            .read()
            .expect("workspace state poisoned")
            .overlays
            .get(&path)
            .cloned()
            .ok_or(WorkspaceError::DocumentNotOpen(path))
    }

    pub fn published(&self) -> Option<Arc<WorkspaceSnapshot>> {
        self.state
            .read()
            .expect("workspace state poisoned")
            .published
            .clone()
    }

    pub async fn rebuild(
        &self,
        context: &QueryContext,
    ) -> Result<Arc<WorkspaceSnapshot>, WorkspaceError> {
        context.checkpoint().await?;
        let overlays = self
            .state
            .read()
            .expect("workspace state poisoned")
            .overlays
            .iter()
            .map(|(path, document)| (path.clone(), document.text().clone()))
            .collect::<BTreeMap<_, _>>();
        let mut snapshot = match self
            .engine
            .recover_workspace_async(&self.root, &overlays, context)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                context.check()?;
                return Err(error.into());
            }
        };
        context.checkpoint().await?;
        snapshot.set_revision(context.revision());
        let snapshot = Arc::new(snapshot);
        let mut state = self.state.write().expect("workspace state poisoned");
        context.check()?;
        state.published = Some(Arc::clone(&snapshot));
        Ok(snapshot)
    }
}

fn canonical_document_path(path: &Path) -> Result<PathBuf, WorkspaceError> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path.is_absolute() {
                Ok(path.to_owned())
            } else {
                Ok(std::env::current_dir()
                    .map_err(WorkspaceError::Io)?
                    .join(path))
            }
        }
        Err(error) => Err(WorkspaceError::Io(error)),
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Document(DocumentError),
    DocumentNotOpen(PathBuf),
    Io(std::io::Error),
    Module(ModuleError),
    Query(QueryError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(error) => fmt::Display::fmt(error, formatter),
            Self::DocumentNotOpen(path) => {
                write!(formatter, "document is not open: {}", path.display())
            }
            Self::Io(error) => fmt::Display::fmt(error, formatter),
            Self::Module(error) => fmt::Display::fmt(error, formatter),
            Self::Query(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<DocumentError> for WorkspaceError {
    fn from(error: DocumentError) -> Self {
        Self::Document(error)
    }
}

impl From<ModuleError> for WorkspaceError {
    fn from(error: ModuleError) -> Self {
        Self::Module(error)
    }
}

impl From<QueryError> for WorkspaceError {
    fn from(error: QueryError) -> Self {
        Self::Query(error)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::{EngineConfig, Quota, TextRange};

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return result;
            }
        }
    }

    fn fixture(source: &str) -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("telora-workspace-test-{unique}"));
        std::fs::create_dir_all(&directory).unwrap();
        let root = directory.join("main.telora");
        std::fs::write(&root, source).unwrap();
        (directory, root)
    }

    fn engine() -> Engine {
        Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(1_000_000),
            session_quota: Quota::with_fuel(1_000_000),
            data_limits: crate::DataLimits::default(),
        })
    }

    #[test]
    fn overlay_revisions_are_cow_and_publish_atomically() {
        let (directory, root) = fixture("let disk = 1; disk");
        let workspace = Workspace::new(&root, engine()).unwrap();
        let old_context = workspace.context();
        let revision = workspace
            .open(&root, DocumentVersion(1), "let overlay = 2; overlay")
            .unwrap();
        assert_eq!(revision, Revision(1));
        assert!(matches!(
            old_context.check(),
            Err(QueryError::StaleRevision { .. })
        ));
        let old_document = workspace.document(&root).unwrap();

        let revision = workspace
            .change(
                &root,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Replace {
                    range: TextRange::new(14, 15).unwrap(),
                    replacement: "3".into(),
                }],
            )
            .unwrap();
        assert_eq!(revision, Revision(2));
        assert_eq!(old_document.text().to_string(), "let overlay = 2; overlay");
        assert_eq!(
            workspace.document(&root).unwrap().text().to_string(),
            "let overlay = 3; overlay"
        );

        let context = workspace.context();
        let snapshot = block_on(workspace.rebuild(&context)).unwrap();
        assert_eq!(snapshot.revision(), Revision(2));
        assert!(
            snapshot
                .definitions()
                .iter()
                .any(|definition| definition.name == "overlay")
        );
        assert!(
            !snapshot
                .definitions()
                .iter()
                .any(|definition| definition.name == "disk")
        );
        assert_eq!(workspace.published().unwrap().revision(), Revision(2));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cancelled_and_stale_builds_do_not_publish() {
        let (directory, root) = fixture("1");
        let workspace = Workspace::new(&root, engine()).unwrap();
        let cancellation = CancellationToken::default();
        let context = workspace.cancellable_context(cancellation.clone());
        cancellation.cancel();
        assert!(matches!(
            block_on(workspace.rebuild(&context)),
            Err(WorkspaceError::Query(QueryError::Cancelled))
        ));
        assert!(workspace.published().is_none());

        let stale = workspace.context();
        workspace.open(&root, DocumentVersion(1), "2").unwrap();
        assert!(matches!(
            block_on(workspace.rebuild(&stale)),
            Err(WorkspaceError::Query(QueryError::StaleRevision { .. }))
        ));
        assert!(workspace.published().is_none());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn valid_overlay_dependencies_supply_real_import_capabilities() {
        let (directory, root) = fixture(
            "import \"./model.telora\" as model; type FromOverlay = model.Shared; export { FromOverlay as output };",
        );
        let model = directory.join("model.telora");
        std::fs::write(&model, "type Shared = missing; 0").unwrap();
        let workspace = Workspace::new(&root, engine()).unwrap();
        workspace
            .open(
                &model,
                DocumentVersion(1),
                "type Shared = String; export { Shared };",
            )
            .unwrap();
        let context = workspace.context();
        let snapshot = block_on(workspace.rebuild(&context)).unwrap();
        let root_module = snapshot
            .module_by_path(&std::fs::canonicalize(&root).unwrap())
            .unwrap();
        let fact = &snapshot
            .definitions()
            .iter()
            .find(|definition| {
                definition.module == root_module.id && definition.name == "FromOverlay"
            })
            .unwrap()
            .ty;
        assert_eq!(fact.state, crate::FactState::Known);
        assert_eq!(
            snapshot.types().display(fact.value.unwrap()).unwrap(),
            "TypeOf(String)"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
