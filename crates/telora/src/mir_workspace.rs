//! Versioned editor input and snapshots over the same MIR pipeline as the CLI.
//! No Engine, legacy semantic snapshot, VM, or alternate type graph.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
};
use telora_core::{
    CancellationToken, DocumentSnapshot, DocumentVersion, QueryContext, QueryError, Revision,
    RevisionClock, TextEdit,
    mir::{Mir, ModuleId, ModuleState},
    mir_query::MirQuery,
};

pub struct Workspace {
    root: PathBuf,
    clock: RevisionClock,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    documents: BTreeMap<PathBuf, DocumentSnapshot>,
    published: Option<Rc<Snapshot>>,
}

pub struct Snapshot {
    pub mir: Mir,
    pub revision: Revision,
    paths: BTreeMap<PathBuf, ModuleId>,
}

impl Snapshot {
    pub fn sources(&self) -> &telora_core::SourceDatabase {
        &self.mir.sources
    }
    pub fn source_by_path(&self, path: &Path) -> Option<telora_core::SourceId> {
        match self.mir.modules[self.module_by_path(path)?.index()].state {
            ModuleState::Source { source, .. } => Some(source),
            _ => None,
        }
    }
    pub fn query(&self) -> MirQuery<'_> {
        MirQuery::new(&self.mir)
    }
    pub fn module_by_path(&self, path: &Path) -> Option<ModuleId> {
        self.paths.get(path).copied()
    }
    pub fn path_by_source(&self, source: telora_core::SourceId) -> Option<&Path> {
        self.paths.iter().find_map(
            |(path, &module)| match self.mir.modules[module.index()].state {
                ModuleState::Source { source: id, .. } if id == source => Some(path.as_path()),
                _ => None,
            },
        )
    }
    pub fn ensure_current(&self, context: &QueryContext) -> Result<(), QueryError> {
        context.ensure_snapshot(self.revision)
    }
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            root: canonical(root.as_ref())?,
            clock: RevisionClock::default(),
            state: RefCell::new(State::default()),
        })
    }
    pub fn revision(&self) -> Revision {
        self.clock.current()
    }
    pub fn context(&self) -> QueryContext {
        QueryContext::current(self.clock.clone())
    }
    pub fn cancellable_context(&self, token: CancellationToken) -> QueryContext {
        QueryContext::new(self.revision(), self.clock.clone(), token)
    }
    pub fn published(&self) -> Option<Rc<Snapshot>> {
        self.state.borrow().published.clone()
    }

    pub fn open(
        &self,
        path: impl AsRef<Path>,
        version: DocumentVersion,
        text: impl AsRef<str>,
    ) -> Result<Revision, Error> {
        self.state.borrow_mut().documents.insert(
            canonical(path.as_ref())?,
            DocumentSnapshot::new(version, text),
        );
        Ok(self.clock.advance())
    }
    pub fn document(&self, path: impl AsRef<Path>) -> Result<DocumentSnapshot, Error> {
        let path = canonical(path.as_ref())?;
        self.state
            .borrow()
            .documents
            .get(&path)
            .cloned()
            .ok_or_else(|| Error::Input(format!("document is not open: {}", path.display())))
    }
    pub fn change(
        &self,
        path: impl AsRef<Path>,
        expected: DocumentVersion,
        version: DocumentVersion,
        edits: &[TextEdit],
    ) -> Result<Revision, Error> {
        let path = canonical(path.as_ref())?;
        let mut state = self.state.borrow_mut();
        let current = state
            .documents
            .get(&path)
            .ok_or_else(|| Error::Input(format!("document is not open: {}", path.display())))?;
        let changed = current
            .changed(expected, version, edits)
            .map_err(|e| Error::Input(e.to_string()))?;
        state.documents.insert(path, changed);
        Ok(self.clock.advance())
    }
    pub fn close(&self, path: impl AsRef<Path>) -> Result<Revision, Error> {
        let path = canonical(path.as_ref())?;
        if self.state.borrow_mut().documents.remove(&path).is_none() {
            return Err(Error::Input(format!(
                "document is not open: {}",
                path.display()
            )));
        }
        Ok(self.clock.advance())
    }

    pub async fn rebuild(&self, context: &QueryContext) -> Result<Rc<Snapshot>, Error> {
        context.checkpoint().await?;
        let documents = self.state.borrow().documents.clone();
        let mut inventory =
            crate::static_input::Inventory::new(&self.root, false).map_err(Error::Input)?;
        let mut roots = vec![inventory.document_name(&self.root).map_err(Error::Input)?];
        let mut overlays = BTreeMap::new();
        for (path, document) in documents {
            context.checkpoint().await?;
            let name = inventory.document_name(&path).map_err(Error::Input)?;
            roots.push(name.clone());
            overlays.insert(name, document.text().clone());
        }
        roots.sort();
        roots.dedup();
        context.checkpoint().await?;
        let mir = inventory.solve_documents(&roots, &overlays);
        context.checkpoint().await?;
        let files = inventory.module_paths();
        let mut paths = BTreeMap::new();
        for (index, module) in mir.modules.iter().enumerate() {
            let Some(path) = files.get(&module.name) else {
                continue;
            };
            // ModuleIds come from the actual graph, never a separately numbered
            // editor module table. Obtain them through the HIR/source owner.
            if let ModuleState::Source { body, .. } | ModuleState::Data { body } = module.state {
                let id = mir.hir[body.index()].module;
                debug_assert_eq!(id.index(), index);
                paths.insert(canonical(path)?, id);
            }
        }
        let snapshot = Rc::new(Snapshot {
            mir,
            revision: context.revision(),
            paths,
        });
        context.check()?;
        self.state.borrow_mut().published = Some(snapshot.clone());
        Ok(snapshot)
    }
}

fn canonical(path: &Path) -> Result<PathBuf, Error> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path.is_absolute() {
                Ok(path.to_owned())
            } else {
                std::env::current_dir()
                    .map(|base| base.join(path))
                    .map_err(|e| Error::Input(e.to_string()))
            }
        }
        Err(error) => Err(Error::Input(error.to_string())),
    }
}

#[derive(Debug)]
pub enum Error {
    Input(String),
    Query(QueryError),
}
impl From<QueryError> for Error {
    fn from(error: QueryError) -> Self {
        Self::Query(error)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(message) => f.write_str(message),
            Self::Query(error) => error.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("telora-config.json"),
            r#"{"version":1,"members":["."]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("telora-crate.json"), r#"{"name":"editor","modules":["@src/main","@src/model","@src/other"],"dependencies":[]}"#).unwrap();
        std::fs::write(
            dir.path().join("src/main.telora"),
            "import \"./model\" as model; export def value = model.value;",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/model.telora"),
            "export def value = 42;",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/other.telora"), "export def other = 1;").unwrap();
        let spec = telora_core::WorkspaceSpec::discover(dir.path()).unwrap();
        spec.write_lock(&spec.generate_lock(&BTreeMap::new()).unwrap())
            .unwrap();
        dir
    }
    fn exported_type(snapshot: &Snapshot, name: &str) -> String {
        let query = snapshot.query();
        let (id, _) = query
            .symbols()
            .find(|(_, symbol)| {
                symbol.name == name && symbol.kind == telora_core::mir::SymbolKind::Export
            })
            .unwrap();
        let telora_core::mir::TypeState::Known(ty) = query.symbol_type(id) else {
            panic!("known type");
        };
        query.type_name(ty)
    }
    #[tokio::test]
    async fn overlays_and_all_open_roots_share_one_static_graph() {
        let dir = fixture();
        let workspace = Workspace::new(dir.path().join("src/main.telora")).unwrap();
        let model = dir.path().join("src/model.telora");
        workspace
            .open(
                &model,
                DocumentVersion(1),
                "export def value = \"overlay\";",
            )
            .unwrap();
        workspace
            .open(
                dir.path().join("src/other.telora"),
                DocumentVersion(1),
                "export def other = missing;",
            )
            .unwrap();
        let snapshot = workspace.rebuild(&workspace.context()).await.unwrap();
        assert_eq!(exported_type(&snapshot, "value"), "String");
        assert_eq!(snapshot.mir.roots.len(), 3);
        assert!(!snapshot.mir.diagnostics.is_empty());
        assert!(snapshot.mir.types_solved && snapshot.mir.symbols_closed);
        assert!(snapshot.module_by_path(&model).is_some());
        let module = snapshot.module_by_path(&model).unwrap();
        let ModuleState::Source { source, .. } = snapshot.mir.modules[module.index()].state else {
            panic!("source");
        };
        assert_eq!(snapshot.path_by_source(source), Some(model.as_path()));
        assert_eq!(
            snapshot
                .mir
                .modules
                .iter()
                .filter(|module| module.name == "editor/model")
                .count(),
            1
        );
        workspace.close(&model).unwrap();
        let fresh = workspace.rebuild(&workspace.context()).await.unwrap();
        assert_eq!(exported_type(&fresh, "value"), "Int");
        assert_eq!(exported_type(&snapshot, "value"), "String");
    }
    #[tokio::test]
    async fn cancelled_and_stale_builds_never_replace_the_published_graph() {
        let dir = fixture();
        let main = dir.path().join("src/main.telora");
        let workspace = Workspace::new(&main).unwrap();
        let previous = workspace.rebuild(&workspace.context()).await.unwrap();
        let stale = workspace.context();
        workspace
            .open(
                &main,
                DocumentVersion(1),
                "export def value = panic!(\"must never execute\");",
            )
            .unwrap();
        assert!(matches!(
            workspace.rebuild(&stale).await,
            Err(Error::Query(QueryError::StaleRevision { .. }))
        ));
        let token = CancellationToken::default();
        let cancelled = workspace.cancellable_context(token.clone());
        let mut building = Box::pin(workspace.rebuild(&cancelled));
        assert!(matches!(
            futures::poll!(&mut building),
            std::task::Poll::Pending
        ));
        token.cancel();
        assert!(matches!(
            building.await,
            Err(Error::Query(QueryError::Cancelled))
        ));
        assert!(Rc::ptr_eq(&workspace.published().unwrap(), &previous));
        let next = workspace.rebuild(&workspace.context()).await.unwrap();
        assert!(next.mir.diagnostics.is_empty());
        assert!(next.ensure_current(&workspace.context()).is_ok());
        assert!(previous.ensure_current(&workspace.context()).is_err());
    }

    #[tokio::test]
    async fn document_changes_are_versioned_and_test_roots_share_catalog_identity() {
        let dir = fixture();
        let tests = dir.path().join("tests");
        std::fs::create_dir(&tests).unwrap();
        let first = tests.join("_first.telora");
        let second = tests.join("second.telora");
        std::fs::write(&first, "export def first = 1;").unwrap();
        std::fs::write(&second, "export def second = 2;").unwrap();
        let workspace = Workspace::new(&first).unwrap();
        workspace
            .open(&first, DocumentVersion(1), "export def first = 1;")
            .unwrap();
        workspace
            .open(&second, DocumentVersion(1), "export def second = 2;")
            .unwrap();
        let revision = workspace.revision();
        assert!(
            workspace
                .change(
                    &first,
                    DocumentVersion(0),
                    DocumentVersion(2),
                    &[TextEdit::Full("invalid".into())]
                )
                .is_err()
        );
        assert_eq!(workspace.revision(), revision);
        assert_eq!(
            workspace.document(&first).unwrap().text().to_string(),
            "export def first = 1;"
        );
        let old_document = workspace.document(&first).unwrap();
        workspace
            .change(
                &first,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Full("export def first = \"new\";".into())],
            )
            .unwrap();
        let snapshot = workspace.rebuild(&workspace.context()).await.unwrap();
        assert_eq!(old_document.text().to_string(), "export def first = 1;");
        assert!(
            snapshot.mir.diagnostics.is_empty(),
            "{:?}",
            snapshot.mir.diagnostics
        );
        assert_eq!(snapshot.mir.roots.len(), 2);
        assert_eq!(exported_type(&snapshot, "first"), "String");
        assert!(snapshot.module_by_path(&first).is_some());
        assert!(snapshot.module_by_path(&second).is_some());
    }
}
