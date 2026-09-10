/// All handles remain private to the VM. Host protocol conversion must borrow
/// the current world, never export state/closures as persistent host values.
struct SolvedRunSession {
    main: Arc<Heap>,
    world: Option<WorkWorld>,
    account: QuotaAccount,
    calls: crate::codegen::RunCalls,
    phase: SolvedRunPhase,
}

#[cfg(test)]
mod solved_run_tests {
    use super::*;

    #[test]
    fn failed_callback_ends_the_session_without_retry_or_quota_reset() {
        let mir = crate::codegen::tests::graph(
            "export def answer = fn(env: Int) { (0, fn(resources: Int) { (0, fn(state: Int, event: Int) { if event == 0 { (state, [0]) } else { fail!(\"event failed\") } }) }) };",
            "",
        );
        let artifact =
            crate::codegen::compile_run(mir.seal().unwrap(), crate::codegen::tests::entry(&mir))
                .unwrap();
        let mut vm = Vm::new();
        let mut session = SolvedRunSession::start(
            &mut vm,
            crate::execution_link::link_entry(artifact).unwrap(),
            Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut SourceDatabase::default(),
        )
        .unwrap();
        let zero = Val::unknown(DecodedValue::Int(0));
        session.configure(&mut vm, zero).unwrap();
        session.initialize(&mut vm, zero).unwrap();
        session.reduce(&mut vm, zero).unwrap();
        let error = session
            .reduce(&mut vm, Val::unknown(DecodedValue::Int(1)))
            .unwrap_err();
        assert!(error.contains("event failed"), "{error}");
        assert!(session.world.is_none());
        assert!(matches!(session.phase, SolvedRunPhase::Failed));
        let remaining = session.account.remaining_fuel;
        assert!(session.reduce(&mut vm, zero).is_err());
        assert_eq!(session.account.remaining_fuel, remaining);
    }

    #[test]
    fn policy_callbacks_reuse_the_world_state_handles_and_quota_account() {
        let mir = crate::codegen::tests::graph(
            r#"
            import "./math" as policy;
            import "std/entry" as entry;
            import "std/ees" as ees;
            import "std/_rt" as rt;
            def input = [42];
            def app = entry.run(Array(Int).type, {sources: [], envs: [], args: False}, ees.none,
                fn(ctx) { (input, fn(state, event) { (state, []) }) });
            def main: policy.MainType = {config: app.config, ees: app.ees, start: app.start};
            export def answer = fn(env: ()) {
                let configured = policy.config({args: [], ees: {}, mode: rt.EntryMode.Run,
                    platform: {os: "linux", arch: "x86_64"}, sources: {}}, main);
                (configured.0, fn(resources: ()) {
                    configured.1({data: {}, texts: {}, vars: {}, stdin: None}, main)
                })
            };
            "#,
            include_str!("../../modules/std/_entry/run.telora"),
        );
        let artifact =
            crate::codegen::compile_run(mir.seal().unwrap(), crate::codegen::tests::entry(&mir))
                .unwrap();
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        let mut vm = Vm::new();
        let mut session = SolvedRunSession::start(
            &mut vm,
            linked,
            Quota::with_fuel(100000),
            crate::DataLimits::default(),
            &mut SourceDatabase::default(),
        )
        .unwrap();
        let heap = &mut session.world.as_mut().unwrap().heap;
        let unit = Val::unknown(DecodedValue::Tuple(
            heap.allocate(Object::Tuple(vec![].into())),
        ));
        let caps = session.configure(&mut vm, unit).unwrap();
        assert!(
            matches!(session.phase, SolvedRunPhase::Initialize { caps: saved, .. } if saved.value() == caps.value())
        );
        assert!(
            session.configure(&mut vm, unit).is_err(),
            "configuration is not repeated"
        );
        session.initialize(&mut vm, unit).unwrap();
        let original = actor_payload(&session);
        let image = session.main.solved_types.as_ref().unwrap().types.as_ptr();
        let event = Val::unknown(
            session
                .world
                .as_mut()
                .unwrap()
                .heap
                .atom(Some(&session.main), "Initialize"),
        )
        .with_type_id(crate::TypeId::solved(session.calls.contract.event));
        let mut previous_fuel = session.account.remaining_fuel;
        for _ in 0..3 {
            let effects = session.reduce(&mut vm, event).unwrap();
            let view = HeapView {
                current: &session.world.as_ref().unwrap().heap,
                background: Some(&session.main),
            };
            assert_eq!(
                (ValueRef {
                    value: effects,
                    view
                })
                .sequence_len(),
                Some(0)
            );
            assert_eq!(actor_payload(&session).value(), original.value());
            assert_eq!(actor_payload(&session).type_id(), original.type_id());
            assert_eq!(
                session.main.solved_types.as_ref().unwrap().types.as_ptr(),
                image
            );
            assert!(
                session.account.remaining_fuel < previous_fuel,
                "event calls share the quota account"
            );
            previous_fuel = session.account.remaining_fuel;
        }
        assert!(
            session.initialize(&mut vm, unit).is_err(),
            "initialization is not repeated"
        );
    }

    fn actor_payload(session: &SolvedRunSession) -> Val {
        let SolvedRunPhase::Reduce { state, .. } = session.phase else {
            panic!("state")
        };
        let view = HeapView {
            current: &session.world.as_ref().unwrap().heap,
            background: Some(&session.main),
        };
        let state = ValueRef { value: state, view }
            .dict_get("service")
            .unwrap()
            .dict_get("state")
            .unwrap()
            .runtime();
        let DecodedValue::Dyn(handle) = state.value() else {
            panic!("Dyn")
        };
        view.dyn_parts(handle).unwrap().2
    }
}

#[derive(Clone, Copy)]
enum SolvedRunPhase {
    Configure,
    Initialize { caps: Val, initializer: Val },
    Reduce { state: Val, reducer: Val },
    Failed,
}

impl SolvedRunSession {
    fn start(
        vm: &mut Vm,
        entry: crate::execution_link::LinkedEntry,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<Self, String> {
        let calls = entry
            .run_calls
            .ok_or("missing compiled run policy adapters")?;
        let mut main = Heap::main();
        main.solved_types = Some(entry.types);
        main.solved_graph = Some(entry.graph);
        let mut account = QuotaAccount::new(quota).with_sources(sources);
        let externals = solved_module_data(&mut main, entry.data, limits, sources, &mut account)?;
        let main = Arc::new(main);
        let world = vm
            .execute_in_work(&main, &externals, &entry.bytecode, &[], &mut account)
            .map_err(|e| e.with_sources(sources).to_string())?;
        Ok(Self {
            main,
            world: Some(world),
            account,
            calls,
            phase: SolvedRunPhase::Configure,
        })
    }

    fn invoke(&mut self, vm: &mut Vm, callable: Val, arguments: &[Val]) -> Result<Val, String> {
        let mut world = self.world.take().ok_or("run session has already failed")?;
        world.root = callable;
        let adapter = match arguments.len() {
            1 => &self.calls.unary,
            2 => &self.calls.binary,
            _ => unreachable!("compiled run callback arity"),
        };
        // This moves the owning Rust container; it does not relocate its heap.
        match vm.execute_in_existing_world_with_runtime_args(
            &self.main,
            &HashMap::new(),
            adapter,
            world,
            arguments,
            &[],
            &mut self.account,
        ) {
            Ok(world) => {
                let value = world.root;
                self.world = Some(world);
                Ok(value)
            }
            Err(error) => {
                self.phase = SolvedRunPhase::Failed;
                Err(error.to_string())
            }
        }
    }

    fn pair(&self, value: Val) -> Result<(Val, Val), String> {
        let world = self
            .world
            .as_ref()
            .ok_or("run session has already failed")?;
        let view = HeapView {
            current: &world.heap,
            background: Some(&self.main),
        };
        let DecodedValue::Tuple(handle) = value.value() else {
            return Err("invalid bytecode: run callback did not return its solved tuple".into());
        };
        let values = view.sequence(handle, true).map_err(|e| e.to_string())?;
        let [first, second] = values else {
            return Err("invalid bytecode: run callback tuple has the wrong arity".into());
        };
        Ok((*first, *second))
    }

    fn configure(&mut self, vm: &mut Vm, env: Val) -> Result<Val, String> {
        if !matches!(self.phase, SolvedRunPhase::Configure) {
            return Err("run session is not awaiting configuration".into());
        }
        let callable = self.world.as_ref().ok_or("failed run session")?.root;
        self.phase = SolvedRunPhase::Failed;
        let result = self.invoke(vm, callable, &[env])?;
        let (caps, initializer) = self.pair(result)?;
        self.phase = SolvedRunPhase::Initialize { caps, initializer };
        Ok(caps)
    }

    fn initialize(&mut self, vm: &mut Vm, resources: Val) -> Result<(), String> {
        let SolvedRunPhase::Initialize { initializer, .. } = self.phase else {
            return Err("run session is not awaiting resources".into());
        };
        self.phase = SolvedRunPhase::Failed;
        let result = self.invoke(vm, initializer, &[resources])?;
        let (state, reducer) = self.pair(result)?;
        self.phase = SolvedRunPhase::Reduce { state, reducer };
        Ok(())
    }

    fn reduce(&mut self, vm: &mut Vm, event: Val) -> Result<Val, String> {
        let SolvedRunPhase::Reduce { state, reducer } = self.phase else {
            return Err("run session is not ready for an event".into());
        };
        self.phase = SolvedRunPhase::Failed;
        let result = self.invoke(vm, reducer, &[state, event])?;
        let (state, effects) = self.pair(result)?;
        self.phase = SolvedRunPhase::Reduce { state, reducer };
        Ok(effects)
    }
}
