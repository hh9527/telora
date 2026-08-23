pub struct Vm {
    debug_sink: Arc<dyn DebugSink>,
}

impl Default for Vm {
    fn default() -> Self {
        Self {
            debug_sink: Arc::new(DiscardDebugSink),
        }
    }
}

struct ExecutionFrame {
    function: Arc<BytecodeFunction>,
    prototype: Handle,
    base: usize,
    pc: usize,
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
}

#[derive(Debug)]
enum ReturnTarget {
    Root,
    Register {
        destination: Register,
        call_site: Option<crate::Loc>,
    },
    Native(Box<dyn NativeContinuation>),
}

trait NativeContinuation: fmt::Debug {
    fn return_target(&self) -> &ReturnTarget;
    fn trace_frame(&self) -> &RuntimeFrame;

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError>;

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError>;
}

#[derive(Debug)]
struct ArrayContinuation {
    function: CoreArrayFunction,
    source: Val,
    callback: Val,
    next_index: usize,
    accumulator: Option<Val>,
    output: Vec<Val>,
    failed: Option<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    trace_frame: RuntimeFrame,
}

#[derive(Debug)]
struct DictContinuation {
    function: CoreDictFunction,
    entries: Vec<(String, Val)>,
    callback: Val,
    next_index: usize,
    accumulator: Option<Val>,
    output: Vec<(String, Val)>,
    failed: Option<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    trace_frame: RuntimeFrame,
}

enum VmAction {
    Call {
        callee: Val,
        arguments: Vec<Val>,
        return_target: ReturnTarget,
        call_function: Arc<BytecodeFunction>,
        call_pc: usize,
        rule_boundary: Option<crate::Loc>,
    },
    Return {
        value: Val,
        return_target: ReturnTarget,
    },
}

enum DriveOutcome {
    Pending,
    Root(Val),
}

pub(crate) struct WorkWorld {
    heap: Heap,
    root: Val,
}

pub(crate) struct VmExecution {
    pub(crate) world: WorkWorld,
    pub(crate) failures: Vec<RuntimeError>,
}

pub(crate) struct VmExecutionFailure {
    heap: Heap,
    pub(crate) error: RuntimeError,
    pub(crate) failures: Vec<RuntimeError>,
}

#[derive(Clone, Copy)]
struct WorkView<'a> {
    main: &'a Heap,
    work: &'a Heap,
}

impl<'a> WorkView<'a> {
    fn heap_view(self) -> HeapView<'a> {
        HeapView {
            current: self.work,
            background: Some(self.main),
        }
    }
}

impl WorkWorld {
    pub(crate) fn root_ref<'a>(&'a self, world: &'a Heap) -> ValueRef<'a> {
        self.value_ref(world, self.root)
    }

    pub(crate) fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    pub(crate) fn heap(&self) -> &Heap {
        &self.heap
    }

    pub(crate) fn value_ref<'a>(&'a self, world: &'a Heap, value: Val) -> ValueRef<'a> {
        ValueRef::work(value, &self.heap, world)
    }

    pub(crate) fn import_world_root(
        mut self,
        background: &Heap,
        source: &WorkWorld,
    ) -> Result<(Self, Val), crate::heap::HeapError> {
        let roots = relocate_work_roots(&mut self.heap, background, &source.heap, &[source.root])?;
        Ok((self, roots[0]))
    }

    pub(crate) fn wrap_root_dyn(
        mut self,
        background: &Heap,
        type_descriptor: &crate::types::TypeDescriptor,
        origin: impl Into<Arc<str>>,
    ) -> Result<Self, crate::heap::HeapError> {
        let descriptor = self
            .heap
            .type_descriptor_value(Some(background), type_descriptor)?;
        self.root = self
            .root
            .with_value(DecodedValue::Dyn(self.heap.allocate(Object::Dyn {
                identity: Arc::new(()),
                descriptor,
                value: self.root,
                scheme: Some(crate::TypeScheme {
                    parameters: Vec::new(),
                    body: type_descriptor.clone(),
                }),
                origin: Some(origin.into()),
            })));
        Ok(self)
    }

    fn module_member(
        &self,
        world: &Heap,
        name: &str,
    ) -> Result<Option<Val>, crate::heap::HeapError> {
        let view = WorkView {
            main: world,
            work: &self.heap,
        }
        .heap_view();
        let DecodedValue::Module(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(
                "execution root is not a Module",
            ));
        };
        let Some(field) = self.heap.find_text(name).or_else(|| world.find_text(name)) else {
            return Ok(None);
        };
        view.exports_get(handle, field)
    }

    pub(crate) fn module_member_ref<'a>(
        &'a self,
        world: &'a Heap,
        name: &str,
    ) -> Result<Option<ValueRef<'a>>, crate::heap::HeapError> {
        self.module_member(world, name)
            .map(|value| value.map(|value| self.value_ref(world, value)))
    }

    pub(crate) fn member_function_arity(
        &self,
        world: &Heap,
        name: &str,
    ) -> Result<Option<usize>, crate::heap::HeapError> {
        let Some(value) = self.module_member(world, name)? else {
            return Ok(None);
        };
        WorkView {
            main: world,
            work: &self.heap,
        }
        .heap_view()
        .resolved_function_arity(value)
    }

    pub(crate) fn seal_module(mut self) -> Result<Self, crate::heap::HeapError> {
        self.root = self.heap.seal_module(self.root)?;
        Ok(self)
    }

    pub(crate) fn module_fields(
        &self,
        world: &Heap,
    ) -> Result<Vec<String>, crate::heap::HeapError> {
        let view = WorkView {
            main: world,
            work: &self.heap,
        }
        .heap_view();
        let DecodedValue::Module(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(
                "execution root is not a Module",
            ));
        };
        view.exports_fields(handle)
            .map(|fields| fields.into_iter().map(str::to_owned).collect())
    }

    pub(crate) fn publish(
        self,
        world: &mut Heap,
    ) -> Result<PersistentValue, crate::heap::HeapError> {
        publish_module_root(world, &self.heap, self.root)
    }

    pub(crate) fn publish_module(
        mut self,
        world: &mut Heap,
    ) -> Result<PersistentValue, crate::heap::HeapError> {
        self.root = self.heap.seal_module(self.root)?;
        publish_module_root(world, &self.heap, self.root)
    }

    pub(crate) fn into_reducer_transition(
        mut self,
        world: &Heap,
    ) -> Result<(Self, Vec<Val>), crate::heap::HeapError> {
        let view = HeapView {
            current: &self.heap,
            background: Some(world),
        };
        let DecodedValue::Tuple(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(
                "Entry reducer must return Tuple([State, Array(SystemEffect)])",
            ));
        };
        let values = view.sequence(handle, true)?;
        let [state, effects] = values else {
            return Err(crate::heap::HeapError::new(
                "Entry reducer transition must contain exactly State and effects",
            ));
        };
        let DecodedValue::Array(effects) = effects.value() else {
            return Err(crate::heap::HeapError::new(
                "Entry reducer effects must be an Array",
            ));
        };
        let effects = view.sequence(effects, false)?.to_vec();
        // Audit the complete batch before the Host observes or executes the
        // first effect. A later failed payload must not permit earlier effects
        // to escape and make the transition partially visible.
        for effect in &effects {
            if view.first_data_failure(*effect)?.is_some() {
                return Err(crate::heap::HeapError::new(
                    "failed evaluation node cannot cross the SystemEffect boundary",
                ));
            }
        }
        self.root = *state;
        Ok((self, effects))
    }

    pub(crate) fn into_runtime_pair(
        mut self,
        world: &Heap,
        root_error: &'static str,
        length_error: &'static str,
    ) -> Result<(Self, Val), crate::heap::HeapError> {
        let view = HeapView {
            current: &self.heap,
            background: Some(world),
        };
        let DecodedValue::Tuple(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(root_error));
        };
        let values = view.sequence(handle, true)?;
        let [state, value] = values else {
            return Err(crate::heap::HeapError::new(length_error));
        };
        let state = *state;
        let value = *value;
        self.root = state;
        Ok((self, value))
    }

    pub(crate) fn runtime_function_arity(
        &self,
        world: &Heap,
        value: Val,
    ) -> Result<Option<usize>, crate::heap::HeapError> {
        HeapView {
            current: &self.heap,
            background: Some(world),
        }
        .resolved_function_arity(value)
    }
}

