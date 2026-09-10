//! Discover deferred tests from the sealed graph, without initializing exports.
//! Runtime consumers receive stable symbols and native type identity, never
//! source names to resolve or values whose types they must guess.
use crate::{
    mir::{
        ModuleId, NativeTypeId, ResolveState, SealedMir, SymbolId, TypeConstructor, TypeId,
        TypeState,
    },
    source::{Diagnostic, Location, Severity},
};

#[derive(Debug)]
pub struct TestExport {
    pub name: String,
    pub symbol: SymbolId,
    pub target: SymbolId,
    pub ty: TypeId,
    pub location: Location,
}

#[derive(Debug)]
pub struct TestPlan {
    pub module: ModuleId,
    pub module_name: String,
    /// Input of the native fixture factory, read from its solved ABI signature.
    pub fixture_type: Option<TypeId>,
    pub exports: Vec<TestExport>,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub fixtures: Vec<usize>,
    pub sources: Vec<String>,
    pub phase: &'static str,
    pub passed: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
pub struct TestReport {
    pub cases: Vec<TestResult>,
    pub notices: Vec<TestNotice>,
    pub diagnostics: Vec<Diagnostic>,
    pub aborted: bool,
}

impl TestReport {
    pub fn passed(&self) -> bool {
        !self.aborted
            && !self.cases.is_empty()
            && self.cases.iter().all(|case| case.passed)
            && !self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
    }
}

#[derive(Debug)]
pub struct TestNotice {
    pub before_case: usize,
    pub context: TestResult,
}

impl TestPlan {
    pub fn from_mir(sealed: &SealedMir<'_>, module: ModuleId) -> Result<Self, Vec<Diagnostic>> {
        let mir = sealed.mir();
        let fail = |message: &str| {
            vec![Diagnostic {
                severity: Severity::Error,
                message: message.into(),
                labels: vec![],
                notes: vec![],
            }]
        };
        let exports = mir
            .exports
            .get(module.index())
            .ok_or_else(|| fail("test module is outside the sealed graph"))?;
        let mut selected = vec![];
        for &symbol in exports {
            let export = &mir.symbols[symbol.index()];
            let ResolveState::Bound(target) = export.resolution else {
                return Err(fail("sealed test export has no resolved target"));
            };
            let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[target.index()].index()]
            else {
                return Err(fail("sealed test export has no solved type"));
            };
            if !mir.symbol_generics[target.index()].is_empty()
                || mir.types[ty.index()].constructor != TypeConstructor::Native(NativeTypeId::TEST)
            {
                continue;
            }
            let declaration = *mir.symbols[target.index()]
                .declarations
                .last()
                .ok_or_else(|| fail("test export has no declaration"))?;
            selected.push(TestExport {
                name: export.name.clone(),
                symbol,
                target,
                ty,
                location: mir.hir[declaration.index()].location,
            });
        }
        selected.sort_by(|a, b| a.name.cmp(&b.name));
        if selected.is_empty() {
            return Err(fail("test module has no direct Test exports"));
        }
        let mut fixture_type = None;
        for symbol in &mir.symbols {
            if symbol.name != "with_fixtures"
                || !symbol.module.is_some_and(|module| {
                    mir.modules[module.index()]
                        .native
                        .as_ref()
                        .is_some_and(|native| native.id == NativeTypeId::TEST.module)
                })
            {
                continue;
            }
            let ResolveState::Bound(target) = symbol.resolution else {
                continue;
            };
            let TypeState::Known(signature) =
                mir.ty_slots[mir.symbol_types[target.index()].index()]
            else {
                continue;
            };
            let signature = &mir.types[signature.index()];
            if signature.constructor != TypeConstructor::Function || signature.arguments.len() != 3
            {
                return Err(fail("invalid native fixture constructor signature"));
            }
            let factory = &mir.types[signature.arguments[1].index()];
            if factory.constructor != TypeConstructor::Function || factory.arguments.len() != 2 {
                return Err(fail("invalid native fixture factory signature"));
            }
            fixture_type = Some(factory.arguments[0]);
            break;
        }
        Ok(Self {
            module,
            module_name: mir.modules[module.index()].name.clone(),
            fixture_type,
            exports: selected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::ModuleTarget;

    #[test]
    fn discovers_native_test_exports_without_running_initializers_or_thunks() {
        let mir = crate::codegen::tests::graph(
            r#"
            import "std/test" as testing;
            type Test = struct {value: Int};
            export def imitation: Test = {value: 1};
            export def factory = fn() { testing.should_ok(fn() { 42 }) };
            export def z_case = testing.should_fail(fn() { fail!("deferred thunk") });
            export def a_case: testing.Test = fail!("must not initialize while planning");
        "#,
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
        let ModuleTarget::Bound(module) = mir.roots[0] else {
            panic!("root");
        };
        let plan = TestPlan::from_mir(&mir.seal().unwrap(), module).unwrap();
        assert_eq!(
            plan.exports
                .iter()
                .map(|case| case.name.as_str())
                .collect::<Vec<_>>(),
            ["a_case", "z_case"]
        );
        for case in &plan.exports {
            assert_eq!(
                mir.types[case.ty.index()].constructor,
                TypeConstructor::Native(NativeTypeId::TEST)
            );
            assert_eq!(
                mir.symbols[case.symbol.index()].resolution,
                ResolveState::Bound(case.target)
            );
        }
    }

    #[test]
    fn reexports_keep_their_resolved_target() {
        let mir = crate::codegen::tests::graph(
            r#"import "./math" {forwarded}; export {forwarded};"#,
            r#"import "std/test" as testing; export def forwarded = testing.should_ok(fn() {42});"#,
        );
        assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
        let ModuleTarget::Bound(module) = mir.roots[0] else {
            panic!("root");
        };
        let plan = TestPlan::from_mir(&mir.seal().unwrap(), module).unwrap();
        assert_eq!(plan.exports.len(), 1);
        let export = &plan.exports[0];
        assert_eq!(export.name, "forwarded");
        assert_ne!(mir.symbols[export.target.index()].module, Some(module));
    }

    #[test]
    fn rejects_modules_without_direct_test_values() {
        let mir = crate::codegen::tests::graph(
            r#"
            import "std/test" as testing;
            export def factory = fn() { testing.should_ok(fn() { 42 }) };
            export def ordinary = 42;
        "#,
            "",
        );
        let ModuleTarget::Bound(module) = mir.roots[0] else {
            panic!("root");
        };
        let errors = TestPlan::from_mir(&mir.seal().unwrap(), module).unwrap_err();
        assert_eq!(errors[0].message, "test module has no direct Test exports");
    }

    #[test]
    fn test_bootstrap_installs_tasks_without_evaluating_test_exports() {
        let mut mir = crate::codegen::tests::graph(
            r#"
            import "std/test" as testing;
            export def first: testing.Test = fail!("do not initialize a test during bootstrap");
            export def second = testing.should_ok(fn() { fail!("do not run a thunk during bootstrap") });
        "#,
            "",
        );
        let ModuleTarget::Bound(module) = mir.roots[0] else {
            panic!("root");
        };
        let compiled = crate::codegen::compile_tests(mir.seal().unwrap(), module).unwrap();
        assert_eq!(compiled.plan.exports.len(), 2);
        for case in &compiled.plan.exports {
            assert!(compiled.bootstrap.graph.global(case.target).is_some());
        }
        let linked = crate::execution_link::link_entry(compiled.bootstrap).unwrap();
        let result = crate::Vm::new()
            .execute_linked(
                linked,
                crate::Quota::with_fuel(10000),
                crate::DataLimits::default(),
                &mut mir.sources,
            )
            .unwrap();
        assert_eq!(result.value().sequence_len(), Some(0));
    }
}
