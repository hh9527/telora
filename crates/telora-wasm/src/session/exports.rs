//! Typed byte-ABI handles belong to one Store/Instance, never to the Module.
use wasmi::{Instance, Store, StoreLimits, TypedFunc};

pub(crate) struct Exports {
    pub alloc: TypedFunc<(u32, u32), u32>,
    pub free: TypedFunc<(u32, u32, u32), ()>,
    pub realloc: TypedFunc<(u32, u32, u32, u32), u32>,
    pub source_count: TypedFunc<(), u32>,
    pub source_name: TypedFunc<(u32, u32), ()>,
    pub set_source: TypedFunc<(u32, u32, u32, u32), ()>,
    pub create_service: TypedFunc<(), i32>,
    pub reset_service: TypedFunc<(), ()>,
    pub diagnostics: TypedFunc<(u32, u32, u32), ()>,
    pub run_service: TypedFunc<(u32, u32, u32, u32, u32), ()>,
}

impl Exports {
    pub fn bind(instance: Instance, store: &Store<StoreLimits>) -> Result<Self, String> {
        let bind = || -> Result<Self, wasmi::Error> {
            Ok(Self {
                alloc: instance.get_typed_func(store, "mem-alloc")?,
                free: instance.get_typed_func(store, "mem-free")?,
                realloc: instance.get_typed_func(store, "mem-realloc")?,
                source_count: instance.get_typed_func(store, "get-data-source-count")?,
                source_name: instance.get_typed_func(store, "get-data-source-name")?,
                set_source: instance.get_typed_func(store, "set-data-source")?,
                create_service: instance.get_typed_func(store, "create-service")?,
                reset_service: instance.get_typed_func(store, "reset-service")?,
                diagnostics: instance.get_typed_func(store, "get-service-diagnostics")?,
                run_service: instance.get_typed_func(store, "run-service")?,
            })
        };
        bind().map_err(|error| error.to_string())
    }
}
