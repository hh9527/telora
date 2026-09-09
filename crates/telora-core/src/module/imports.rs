#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImportId(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImportTarget {
    Pending,
    Resolved(ModuleId),
    Conflicted(u32),
}

// Source lookup is an ingress index; the graph's stable import identities and
// mutable facts live in the array. Refining a fact never replaces its identity.
#[derive(Clone, Debug, Default)]
struct ImportGraph {
    nodes: Vec<ImportTarget>,
    by_location: HashMap<crate::Location, ImportId>,
    diagnostics: Vec<String>,
}

impl ImportGraph {
    fn register(&mut self, location: crate::Location) -> ImportId {
        *self.by_location.entry(location).or_insert_with(|| {
            let id = ImportId(u32::try_from(self.nodes.len()).expect("import count exceeds u32"));
            self.nodes.push(ImportTarget::Pending);
            id
        })
    }

    fn solve(&mut self, id: ImportId, target: Result<ModuleId, String>) {
        let state = match target {
            Ok(target) => ImportTarget::Resolved(target),
            Err(message) => {
                let diagnostic = u32::try_from(self.diagnostics.len())
                    .expect("import diagnostic count exceeds u32");
                self.diagnostics.push(message);
                ImportTarget::Conflicted(diagnostic)
            }
        };
        let slot = &mut self.nodes[id.0 as usize];
        // Multiple selected-member imports can share one source path span.
        // They must resolve to the same target; neither result renumbers a node.
        assert!(*slot == ImportTarget::Pending || *slot == state);
        *slot = state;
    }

    fn target(&self, location: crate::Location) -> Option<Result<ModuleId, &str>> {
        let id = self.by_location.get(&location)?;
        Some(match self.nodes[id.0 as usize] {
            ImportTarget::Pending => Err("module import target is still unresolved"),
            ImportTarget::Resolved(target) => Ok(target),
            ImportTarget::Conflicted(diagnostic) => Err(&self.diagnostics[diagnostic as usize]),
        })
    }
}
