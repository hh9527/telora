//! Service lifecycle is Rust code. Generated artifacts supply only closed
//! layouts and callbacks; no Host service handles or runtime type inference.
use crate::{abi::*, service_sources as inputs, tables, values::{word, telora_invoke}};
use telora_wasm_shared::service::Contract;
mod buffers;
mod run;
mod diagnostics;

#[derive(Clone, Copy, PartialEq)]
enum Phase { Unprepared, Preparing, Prepared, Failed, Ready }

struct Service {
    contract: Contract,
    phase: Phase,
    initializer: u32,
    handler: u32,
    errors: alloc::vec::Vec<alloc::string::String>,
    parse_errors: alloc::vec::Vec<alloc::string::String>,
}
static mut SERVICE: Option<Service> = None;

pub(crate) unsafe fn collect_initialization(gc: &mut crate::collect::Collector) {
    unsafe {
        if let Some(service) = (&mut *core::ptr::addr_of_mut!(SERVICE)).as_mut() {
            assert!(service.phase == Phase::Ready);
            service.handler = gc.value(service.handler, service.contract.handler_type);
            service.initializer = 0;
            inputs::release_initialization_values();
        }
    }
}

pub(crate) unsafe fn snapshot() -> u32 {
    unsafe {
        let service = service();
        assert!(service.phase == Phase::Ready);
        service.handler
    }
}

pub(crate) unsafe fn restore(handler: u32) {
    unsafe {
        let service = service();
        assert!(matches!(service.phase, Phase::Unprepared));
        service.phase = Phase::Ready;
        service.initializer = 0;
        service.handler = handler;
        service.errors.clear();
        service.parse_errors.clear();
    }
}

unsafe fn service() -> &'static mut Service {
    unsafe { (&mut *core::ptr::addr_of_mut!(SERVICE)).as_mut().expect("not a service artifact") }
}

/// Only after a completed call and after Host has consumed/freed its buffers.
/// Traps require instance restoration rather than re-entry into Rust cleanup.
#[unsafe(export_name = "reset-service")]
pub unsafe extern "C" fn reset() {
    unsafe {
        assert!(service().phase == Phase::Ready);
        tables::reset();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_bootstrap(pointer: u32) {
    unsafe {
        assert!((&*core::ptr::addr_of!(SERVICE)).is_none());
        let contract = (pointer as *const Contract).read_unaligned();
        *core::ptr::addr_of_mut!(SERVICE) = Some(Service {
            contract, phase: Phase::Unprepared, initializer: 0, handler: 0,
            errors: alloc::vec::Vec::new(), parse_errors: alloc::vec::Vec::new(),
        });
    }
}

unsafe fn prepare() -> bool {
    unsafe {
        match service().phase {
            Phase::Prepared | Phase::Ready => return true,
            Phase::Failed => return false,
            Phase::Preparing => panic!("recursive service preparation"),
            Phase::Unprepared => {},
        }
        service().phase = Phase::Preparing;
        let contract = service().contract;
        let initialize: unsafe extern "C" fn() -> u32 = core::mem::transmute(contract.initialize);
        let entry: unsafe extern "C" fn() -> u32 = core::mem::transmute(contract.entry);
        if initialize() == 0 { service().phase = Phase::Failed; return false; }
        let plan = entry();
        if plan == 0 { service().phase = Phase::Failed; return false; }
        let fields = word(plan, DATA).checked_add(8).unwrap();
        inputs::telora_service_sources_prepare(fields + contract.names_offset);
        service().initializer = fields + contract.initializer_offset;
        service().phase = Phase::Prepared;
        true
    }
}

#[unsafe(export_name = "get-data-source-count")]
pub unsafe extern "C" fn count() -> u32 {
    unsafe { if prepare() { inputs::telora_service_source_count() } else { 0 } }
}

/// Result storage is a borrowed, four-byte-aligned region of three u32 words.
#[unsafe(export_name = "get-data-source-name")]
pub unsafe extern "C" fn name(index: u32, result: u32) {
    unsafe {
        assert!(prepare());
        buffers::range(result, 12, 4);
        let source = inputs::telora_service_source_name(index);
        core::ptr::copy_nonoverlapping(source as *const u8, result as *mut u8, 12);
    }
}

#[unsafe(export_name = "set-data-source")]
pub unsafe extern "C" fn set_source(id: u32, pointer: u32, length: u32, format: u32) {
    unsafe {
        if !prepare() { return; }
        assert!(service().phase == Phase::Prepared);
        let packet = inputs::telora_service_source_parse(id, pointer, length, format);
        let error = word(packet, 12);
        if error != 0 {
            let diagnostics = crate::data_parse::error_text(error + 8);
            service().parse_errors.push(alloc::string::String::from(diagnostics));
            service().phase = Phase::Failed;
            return;
        }
        let materialize: unsafe extern "C" fn(u32, u32) -> u32 =
            core::mem::transmute(service().contract.materialize);
        let value = materialize(packet, 0);
        inputs::telora_service_source_store(id, value);
        if value == 0 { service().phase = Phase::Failed; }
    }
}

#[unsafe(export_name = "create-service")]
pub unsafe extern "C" fn create() -> i32 {
    unsafe {
        if !prepare() { return 1; }
        assert!(service().phase == Phase::Prepared, "service already created");
        service().phase = Phase::Failed;
        let contract = service().contract;
        let missing = inputs::missing();
        if !missing.is_empty() {
            service().errors.extend(missing);
            return 1;
        }
        let context = inputs::context::telora_service_context(contract.context_type,
            contract.dict_type, contract.value_bytes, contract.sources_offset,
            contract.string_type, contract.value_type);
        if context == 0 { return 1; }
        let args = crate::telora_alloc(8);
        crate::heap::write(args, context);
        let handler = telora_invoke(service().initializer, args);
        if handler == 0 { return 1; }
        service().handler = handler;
        service().phase = Phase::Ready;
        tables::telora_freeze();
        0
    }
}
