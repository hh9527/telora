use crate::{service::ServiceSession, session::Session};
use telora_core::{entry_plan, mir::TypeState};

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
    let mut session = Session::load(&bytes, 1_000_000).unwrap();
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
    for expected in [13, 14, 15] {
        let event = service.session_mut().input_value(int, &1.into()).unwrap();
        let effects = service.reduce(event).unwrap();
        assert_eq!(
            service.session().output_value(effects).unwrap(),
            serde_json::json!([expected])
        );
        let remaining = service.session().store.get_fuel().unwrap();
        assert!(remaining < fuel);
        fuel = remaining;
    }
    assert!(service.reduce(zero).is_err());
    let fuel = service.session().store.get_fuel().unwrap();
    assert!(service.reduce(zero).is_err());
    assert_eq!(service.session().store.get_fuel().unwrap(), fuel);
    let diagnostics = service.session().diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "service event failed");
}
