use crate::{service::ServiceSession, session::Session};
use telora_core::{entry_plan, mir::TypeState};

#[test]
fn collection_retains_growing_service_state_and_reclaims_it_after_reset() {
    let mir = super::graph(include_str!(
        "../../tests/fixtures/service-growing-state.telora"
    ));
    let symbol = *mir
        .exports
        .iter()
        .flatten()
        .find(|id| mir.symbols[id.index()].name == "configure")
        .unwrap();
    let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else {
        panic!("closed configure")
    };
    let sealed = mir.seal().unwrap();
    let contract = entry_plan::run_contract(sealed.types(), ty).unwrap();
    let bytes = crate::compile_executable(&sealed.seal_export(symbol).unwrap()).unwrap();
    let mut session = Session::load(&bytes, 1_000_000_000).unwrap();
    session.initialize().unwrap();
    let mut service = ServiceSession::new(session, contract).unwrap();
    let int = contract.env.index() as u32;
    let env = service.session_mut().input_value(int, &0.into()).unwrap();
    service.configure(env).unwrap();
    let resources = service.session_mut().input_value(int, &0.into()).unwrap();
    service.initialize(resources).unwrap();
    let (_, baseline) = service.collect(&[]).unwrap();
    let mut expected = vec![0];
    let mut previous = baseline.heap_after;
    for n in 1..=128 {
        let event = service.session_mut().input_value(int, &n.into()).unwrap();
        let effects = service.reduce(event).unwrap();
        expected.push(n);
        let (roots, stats) = service.collect(&[effects]).unwrap();
        // Verify the whole history after relocation, including the extra Host root.
        assert_eq!(
            service.session().output_value(roots[0]).unwrap(),
            serde_json::json!(expected)
        );
        assert!(stats.heap_after > previous);
        previous = stats.heap_after;
    }
    let event = service
        .session_mut()
        .input_value(int, &(-1).into())
        .unwrap();
    let effects = service.reduce(event).unwrap();
    assert_eq!(
        service.session().output_value(effects).unwrap(),
        serde_json::json!([])
    );
    let (_, reset) = service.collect(&[]).unwrap();
    assert!(reset.heap_after <= baseline.heap_after);
    let event = service.session_mut().input_value(int, &42.into()).unwrap();
    let effects = service.reduce(event).unwrap();
    assert_eq!(
        service.session().output_value(effects).unwrap(),
        serde_json::json!([42])
    );
}

#[test]
fn collection_preserves_shared_graphs_resources_and_interpreter_cycles() {
    let bytes = super::compile(include_str!("../../tests/fixtures/collection.telora")).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let factory = crate::transport::Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let argument_type = session.manifest.types[factory.ty as usize].arguments[0];
    let argument = session.input_value(argument_type, &42.into()).unwrap();
    let mut closure = session.invoke_values(factory, &[argument]).unwrap();
    let result = session.invoke_values(closure, &[]).unwrap();
    let expected = session.output_value(result).unwrap();
    assert_eq!(
        expected,
        serde_json::json!([
            true,
            true,
            true,
            true,
            "[a long shared string é🦀]",
            true,
            42,
            42,
            42,
            true
        ])
    );
    let mut plateau = None;
    for _ in 0..12 {
        let (roots, stats) = session.collect_work(&[closure]).unwrap();
        closure = roots[0];
        if let Some(end) = plateau {
            assert_eq!(stats.heap_after, end);
        }
        plateau = Some(stats.heap_after);
        let result = session.invoke_values(closure, &[]).unwrap();
        assert_eq!(session.output_value(result).unwrap(), expected);
    }
}

#[test]
fn service_keeps_state_and_fuel_and_stops_after_failure() {
    let mir = super::graph(include_str!("../../tests/fixtures/service-state.telora"));
    let symbol = *mir
        .exports
        .iter()
        .flatten()
        .find(|id| mir.symbols[id.index()].name == "configure")
        .unwrap();
    let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else {
        panic!("closed configure")
    };
    let sealed = mir.seal().unwrap();
    let contract = entry_plan::run_contract(sealed.types(), ty).unwrap();
    let bytes = crate::compile_executable(&sealed.seal_export(symbol).unwrap()).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let mut service = ServiceSession::new(session, contract).unwrap();
    let int = contract.env.index() as u32;
    let zero = service.session_mut().input_value(int, &0.into()).unwrap();
    assert!(service.reduce(zero).is_err());
    let env = service.session_mut().input_value(int, &10.into()).unwrap();
    let caps = service.configure(env).unwrap();
    assert_eq!(
        service.session().output_value(caps).unwrap(),
        serde_json::json!(10)
    );
    assert!(service.configure(zero).is_err());
    let resources = service.session_mut().input_value(int, &2.into()).unwrap();
    service.initialize(resources).unwrap();
    let mut fuel = service.session().store.get_fuel().unwrap();
    let mut plateau = None;
    let mut memory_plateau = None;
    for expected in 13..=4108 {
        let event = service.session_mut().input_value(int, &1.into()).unwrap();
        let effects = service.reduce(event).unwrap();
        assert_eq!(
            service.session().output_value(effects).unwrap(),
            serde_json::json!([expected])
        );
        let remaining = service.session().store.get_fuel().unwrap();
        assert!(remaining < fuel);
        fuel = remaining;
        let (_, stats) = service.collect(&[]).unwrap();
        assert!(stats.heap_after < stats.heap_before);
        if let Some(size) = plateau {
            assert_eq!(stats.heap_after, size);
        }
        plateau = Some(stats.heap_after);
        if expected >= 76 {
            if let Some(bytes) = memory_plateau {
                assert_eq!(stats.memory_bytes, bytes);
            }
            memory_plateau = Some(stats.memory_bytes);
        }
    }
    let zero = service.session_mut().input_value(int, &0.into()).unwrap();
    assert!(service.reduce(zero).is_err());
    let fuel = service.session().store.get_fuel().unwrap();
    assert!(service.reduce(zero).is_err());
    assert_eq!(service.session().store.get_fuel().unwrap(), fuel);
    let diagnostics = service.session().diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "service event failed");
}
