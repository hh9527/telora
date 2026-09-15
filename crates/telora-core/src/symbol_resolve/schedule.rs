use super::*;
use std::collections::{BTreeSet, VecDeque};

#[derive(Default)]
pub(super) struct Scheduler {
    ready: VecDeque<ResolveTask>,
    queued: BTreeSet<ResolveTask>,
    consumers: BTreeMap<ResolveTask, Vec<ResolveTask>>,
    #[cfg(test)]
    pub(super) newest_first: bool,
}

impl Scheduler {
    fn next(&mut self) -> Option<ResolveTask> {
        #[cfg(test)]
        if self.newest_first {
            return self.ready.pop_back();
        }
        self.ready.pop_front()
    }
    pub(super) fn enqueue(&mut self, task: ResolveTask) {
        if self.queued.insert(task) {
            self.ready.push_back(task);
        }
    }
}

impl Pass<'_> {
    fn finished(&self, task: ResolveTask) -> bool {
        match task {
            ResolveTask::Symbol(id) => {
                self.mir.symbols[id.index()].resolution != ResolveState::Pending
            }
            ResolveTask::Reference(node) => {
                let slot = self.mir.hir[node.index()].resolution.unwrap();
                self.mir.resolve_slots[slot.index()] != ResolveState::Pending
            }
            ResolveTask::Namespace(id) => {
                self.mir.resolution_facts.namespaces[id.index()].is_some()
            }
            ResolveTask::Constructor(id) => {
                self.mir.resolution_facts.constructors[id.index()].is_some()
            }
            ResolveTask::ConstructorNamespace(id) => {
                self.mir.resolution_facts.constructor_namespaces[id.index()].is_some()
            }
        }
    }

    fn step(&mut self, task: ResolveTask) -> Result<(), ResolveTask> {
        match task {
            ResolveTask::Symbol(id) => {
                let result = self.symbol_step(id)?;
                self.mir.symbols[id.index()].resolution = result;
            }
            ResolveTask::Reference(node) => {
                let result = self.reference_step(node)?;
                let slot = self.mir.hir[node.index()].resolution.unwrap();
                self.mir.resolve_slots[slot.index()] = result;
            }
            ResolveTask::Namespace(id) => {
                let result = self.namespace_step(id)?;
                self.mir.resolution_facts.namespaces[id.index()] = Some(result);
            }
            ResolveTask::Constructor(id) => {
                let result = self.constructor_step(id)?;
                self.mir.resolution_facts.constructors[id.index()] = Some(result);
            }
            ResolveTask::ConstructorNamespace(id) => {
                let result = self.constructor_namespace_step(id)?;
                self.mir.resolution_facts.constructor_namespaces[id.index()] = Some(result);
            }
        }
        Ok(())
    }

    fn notify(&mut self, task: ResolveTask) {
        self.mir.resolution_facts.waiting.remove(&task);
        if let Some(consumers) = self.scheduler.consumers.remove(&task) {
            for consumer in consumers {
                if self.mir.resolution_facts.waiting.get(&consumer) == Some(&task) {
                    self.mir.resolution_facts.waiting.remove(&consumer);
                    self.scheduler.enqueue(consumer);
                }
            }
        }
    }

    pub(super) fn drain(&mut self) {
        loop {
            while let Some(task) = self.scheduler.next() {
                self.scheduler.queued.remove(&task);
                if self.finished(task) {
                    self.notify(task);
                    continue;
                }
                if self.mir.resolution_facts.waiting.contains_key(&task) {
                    continue;
                }
                match self.step(task) {
                    Ok(()) => self.notify(task),
                    Err(dependency) => {
                        // Reads and registration happen in this single-threaded
                        // turn; no producer can finish between them.
                        assert!(!self.finished(dependency));
                        self.mir.resolution_facts.waiting.insert(task, dependency);
                        self.scheduler
                            .consumers
                            .entry(dependency)
                            .or_default()
                            .push(task);
                        if !self.mir.resolution_facts.waiting.contains_key(&dependency) {
                            self.scheduler.enqueue(dependency);
                        }
                    }
                }
            }
            if self.mir.resolution_facts.waiting.is_empty() {
                break;
            }
            // Only quiescence establishes an ungrounded cycle. Each task waits
            // for its first missing input, so the wait graph has outdegree one.
            // Find its cycles iteratively; acyclic consumers remain pending and
            // resume normally when the cycle's negative fact is published.
            let cycles = cycles(&self.mir.resolution_facts.waiting);
            assert!(
                !cycles.is_empty(),
                "every pending producer must be scheduled"
            );
            for cycle in cycles {
                // A negative classification is enough to break a query cycle;
                // prefer it to discarding a reference's existing evidence.
                let task = cycle
                    .into_iter()
                    .min_by_key(|task| {
                        (
                            matches!(task, ResolveTask::Symbol(_) | ResolveTask::Reference(_)),
                            *task,
                        )
                    })
                    .unwrap();
                match task {
                    ResolveTask::Namespace(id) => {
                        self.mir.resolution_facts.namespaces[id.index()] = Some(None)
                    }
                    ResolveTask::Constructor(id) => {
                        self.mir.resolution_facts.constructors[id.index()] = Some(false)
                    }
                    ResolveTask::ConstructorNamespace(id) => {
                        self.mir.resolution_facts.constructor_namespaces[id.index()] = Some(false)
                    }
                    ResolveTask::Symbol(id) => {
                        self.mir.symbols[id.index()].resolution = ResolveState::Unresolved
                    }
                    ResolveTask::Reference(node) => {
                        let slot = self.mir.hir[node.index()].resolution.unwrap();
                        self.mir.resolve_slots[slot.index()] = ResolveState::Unresolved;
                    }
                }
                self.notify(task);
            }
        }
    }
}

fn cycles(waiting: &BTreeMap<ResolveTask, ResolveTask>) -> Vec<Vec<ResolveTask>> {
    let mut visited = BTreeSet::new();
    let mut result = vec![];
    for &start in waiting.keys() {
        if visited.contains(&start) {
            continue;
        }
        let mut path = vec![];
        let mut positions = BTreeMap::new();
        let mut task = start;
        loop {
            if let Some(&index) = positions.get(&task) {
                result.push(path[index..].to_vec());
                break;
            }
            if !visited.insert(task) {
                break;
            }
            positions.insert(task, path.len());
            path.push(task);
            let Some(&next) = waiting.get(&task) else {
                break;
            };
            task = next;
        }
    }
    result
}
