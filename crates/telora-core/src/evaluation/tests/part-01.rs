    #[test]
    fn outcome_map_preserves_never_without_running_user_code() {
        let mut called = false;
        let outcome = EvalOutcome::<i32>::Never(FailureId(4)).map(|value| {
            called = true;
            value + 1
        });
        assert!(!called);
        assert_eq!(outcome, EvalOutcome::Never(FailureId(4)));
    }

    #[test]
    fn propagation_is_stable_deduplicated_and_interned() {
        let mut arena = FailureArena::new(limits());
        let first = root(&mut arena, "first");
        let second = root(&mut arena, "second");
        let location = Some(location(3));
        let propagated =
            arena.propagate_causes(FailureOperation::Binary, location, [first, second, first]);
        let reused =
            arena.propagate_causes(FailureOperation::Binary, location, [first, second, first]);
        assert_eq!(propagated, reused);
        assert_eq!(
            arena.node(propagated),
            Some(&FailureNode::Propagated {
                operation: FailureOperation::Binary,
                location,
                causes: vec![first, second].into_boxed_slice(),
            })
        );
    }

    #[test]
    fn aliases_reuse_ids_and_operation_propagation_collects_never_inputs() {
        let mut arena = FailureArena::new(limits());
        let failure = root(&mut arena, "bad input");
        let alias = EvalOutcome::<i32>::Never(failure);
        assert_eq!(alias.failure(), Some(failure));
        let inputs = [EvalOutcome::Value(1), alias, EvalOutcome::Never(failure)];
        let propagated = arena
            .propagate(FailureOperation::Call, Some(location(2)), &inputs)
            .unwrap();
        let id = propagated.failure().unwrap();
        let FailureNode::Propagated { causes, .. } = arena.node(id).unwrap() else {
            panic!("expected propagation node")
        };
        assert_eq!(causes.as_ref(), &[failure]);
    }

    #[test]
    fn terminal_failures_cannot_enter_the_arena() {
        let mut arena = FailureArena::new(limits());
        assert_eq!(
            arena.root(FailureClass::Terminal, "cancelled"),
            Err("cancelled")
        );
        assert!(arena.nodes.is_empty());
    }

    #[test]
    fn propagation_and_render_depth_budgets_truncate_deterministically() {
        let mut arena = FailureArena::new(FailureLimits::new(1, 2, 2));
        let first = root(&mut arena, "first");
        let second = root(&mut arena, "second");
        let one = arena.propagate_causes(
            FailureOperation::Binary,
            Some(location(1)),
            [first, second, first],
        );
        let truncated =
            arena.propagate_causes(FailureOperation::Call, Some(location(2)), [one, second]);
        let reused = arena.propagate_causes(FailureOperation::Field, Some(location(3)), [second]);
        assert_eq!(truncated, reused);
        assert!(matches!(
            arena.node(truncated),
            Some(FailureNode::Truncated { .. })
        ));
        assert_eq!(
            arena.lineage(truncated),
            vec![
                LineageStep::Truncated,
                LineageStep::Propagated {
                    operation: FailureOperation::Binary,
                    location: Some(location(1)),
                },
                LineageStep::Truncated
            ]
        );
    }

    #[test]
    fn every_propagation_category_is_a_distinct_stable_value() {
        assert_ne!(EvaluationPolicy::Strict, EvaluationPolicy::BestEffort);
        let operations = [
            FailureOperation::Unary,
            FailureOperation::Binary,
            FailureOperation::Field,
            FailureOperation::Index,
            FailureOperation::Call,
            FailureOperation::NativeCall,
            FailureOperation::Condition,
            FailureOperation::Match,
            FailureOperation::Array,
            FailureOperation::Tuple,
            FailureOperation::Tagged,
            FailureOperation::Dict,
            FailureOperation::Interpolation,
            FailureOperation::Binding,
            FailureOperation::ModuleResult,
            FailureOperation::Other,
        ];
        let unique = operations
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), operations.len());
    }

    fn unit(id: u32, kind: EvaluationUnitKind, dependencies: &[u32]) -> EvaluationUnit {
        EvaluationUnit {
            id: EvaluationUnitId(id),
            kind,
            location: location(id),
            dependencies: dependencies.iter().copied().map(EvaluationUnitId).collect(),
        }
    }

    #[test]
    fn plans_require_stable_prior_dependencies_and_one_final_result() {
        assert_eq!(
            EvaluationPlan::new(Vec::new()),
            Err(EvaluationPlanError::Empty)
        );
        assert_eq!(
            EvaluationPlan::new(vec![unit(1, EvaluationUnitKind::ModuleResult, &[])]),
            Err(EvaluationPlanError::NonSequentialId)
        );
        assert_eq!(
            EvaluationPlan::new(vec![
                unit(0, EvaluationUnitKind::Binding, &[]),
                unit(1, EvaluationUnitKind::ModuleResult, &[1]),
            ]),
            Err(EvaluationPlanError::DependencyNotPrior)
        );
        assert_eq!(
            EvaluationPlan::new(vec![
                unit(0, EvaluationUnitKind::Binding, &[]),
                unit(1, EvaluationUnitKind::ModuleResult, &[0, 0]),
            ]),
            Err(EvaluationPlanError::DuplicateDependency)
        );
        assert_eq!(
            EvaluationPlan::new(vec![
                unit(0, EvaluationUnitKind::Binding, &[]),
                unit(1, EvaluationUnitKind::Binding, &[]),
                unit(2, EvaluationUnitKind::ModuleResult, &[1, 0]),
            ]),
            Err(EvaluationPlanError::DependencyNotOrdered)
        );
        assert_eq!(
            EvaluationPlan::new(vec![unit(0, EvaluationUnitKind::Binding, &[])]),
            Err(EvaluationPlanError::MissingModuleResult)
        );
        assert_eq!(
            EvaluationPlan::new(vec![
                unit(0, EvaluationUnitKind::ModuleResult, &[]),
                unit(1, EvaluationUnitKind::Binding, &[]),
            ]),
            Err(EvaluationPlanError::ModuleResultNotLast)
        );
    }

    #[test]
    fn scheduler_continues_independent_units_and_skips_dependents() {
        let plan = EvaluationPlan::new(vec![
            unit(0, EvaluationUnitKind::Binding, &[]),
            unit(1, EvaluationUnitKind::Binding, &[]),
            unit(2, EvaluationUnitKind::ContainerChild, &[0]),
            unit(3, EvaluationUnitKind::ModuleResult, &[1, 2]),
        ])
        .unwrap();
        let mut executed = Vec::new();
        let session = BestEffortSession::run(
            &plan,
            limits(),
            4,
            || Ok::<_, &'static str>(()),
            |unit, dependencies| {
                executed.push(unit.id.0);
                if unit.id.0 == 0 {
                    Err(UnitFailure::new(FailureClass::Recoverable, "bad zero"))
                } else {
                    Ok(dependencies.iter().map(|value| **value).sum::<i32>() + 1)
                }
            },
        )
        .unwrap();
        assert_eq!(executed, vec![0, 1]);
        assert_eq!(session.root_failures().len(), 1);
        assert!(matches!(session.states()[0], EvaluationUnitState::Never(_)));
        assert_eq!(session.states()[1], EvaluationUnitState::Value(1));
        assert!(matches!(session.states()[2], EvaluationUnitState::Never(_)));
        assert!(matches!(session.states()[3], EvaluationUnitState::Never(_)));
        assert!(session.output().is_none());
    }

    #[test]
    fn scheduler_exposes_only_a_clean_complete_result() {
        let plan = EvaluationPlan::new(vec![
            unit(0, EvaluationUnitKind::DefinitionGroup, &[]),
            unit(1, EvaluationUnitKind::Metadata, &[0]),
            unit(2, EvaluationUnitKind::ModuleResult, &[1]),
        ])
        .unwrap();
        let session = BestEffortSession::run(
            &plan,
            limits(),
            4,
            || Ok::<_, &'static str>(()),
            |unit, dependencies| {
                Ok::<_, UnitFailure<&'static str>>(
                    dependencies.first().map_or(unit.id.0, |value| **value + 1),
                )
            },
        )
        .unwrap();
        assert_eq!(session.output(), Some(&2));
        assert!(session.root_failures().is_empty());
        assert!(
            session
                .arena()
                .lineage(FailureId(99))
                .contains(&LineageStep::Truncated)
        );
    }

    #[test]
    fn terminal_failure_and_cancellation_abort_the_session() {
        let plan =
            EvaluationPlan::new(vec![unit(0, EvaluationUnitKind::ModuleResult, &[])]).unwrap();
        let terminal = BestEffortSession::<u32, _>::run(
            &plan,
            limits(),
            1,
            || Ok(()),
            |_, _| Err(UnitFailure::new(FailureClass::Terminal, "quota")),
        );
        assert!(matches!(terminal, Err("quota")));

        let mut checkpoints = 0;
        let cancelled = BestEffortSession::<u32, _>::run(
            &plan,
            limits(),
            1,
            || {
                checkpoints += 1;
                Err("cancelled")
            },
            |_, _| Ok(0),
        );
        assert!(matches!(cancelled, Err("cancelled")));
        assert_eq!(checkpoints, 1);
    }

    #[test]
    fn root_budget_stops_before_starting_another_unit() {
        let plan = EvaluationPlan::new(vec![
            unit(0, EvaluationUnitKind::Binding, &[]),
            unit(1, EvaluationUnitKind::ModuleResult, &[]),
        ])
        .unwrap();
        let mut executions = 0;
        let session = BestEffortSession::<u32, _>::run(
            &plan,
            limits(),
            1,
            || Ok(()),
            |_, _| {
                executions += 1;
                Err(UnitFailure::new(FailureClass::Recoverable, "bad"))
            },
        )
        .unwrap();
        assert_eq!(executions, 1);
        assert!(session.root_budget_exhausted());
        assert!(matches!(session.states()[1], EvaluationUnitState::Pending));
        assert!(session.output().is_none());
    }
