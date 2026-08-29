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
    packages: Option<Arc<crate::package::ResolvedWorkspace>>,
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
            packages: None,
            clock: RevisionClock::default(),
            state: RwLock::new(WorkspaceState::default()),
        })
    }

    pub fn new_in_workspace(
        root: impl AsRef<Path>,
        engine: Engine,
        packages: Arc<crate::package::ResolvedWorkspace>,
    ) -> Result<Self, WorkspaceError> {
        let mut workspace = Self::new(root, engine)?;
        workspace.packages = Some(packages);
        Ok(workspace)
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
        let recovered = if let Some(packages) = &self.packages {
            self.engine
                .recover_workspace_async_in_workspace(
                    Arc::clone(packages),
                    &self.root,
                    &overlays,
                    context,
                )
                .await
        } else {
            self.engine
                .recover_workspace_async(&self.root, &overlays, context)
                .await
        };
        let mut snapshot = match recovered {
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
#[path = "workspace/tests/mod.rs"]
mod tests;
