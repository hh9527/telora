use wasmi::{Config, CustomFuelCosts, Engine, Linker, Module, Store, StoreLimitsBuilder};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let mib = 1024 * 1024;
    let memory: usize = a[2].parse::<usize>()? * mib;
    let heap: u32 = a[3].parse::<u32>()? * mib as u32;
    let interval: u32 = a[4].parse::<u32>()? * 1024;
    let fuel: u64 = a[5].parse::<u64>()? * 1_000_000;
    let mut c = Config::default();
    c.consume_fuel(true);
    c.fuel_cost(CustomFuelCosts {
        bytes_copied_per_fuel: 64,
        fuel_per_bytes_translated: 0,
        fuel_per_bytes_validated: 0,
    });
    let engine = Engine::new(&c);
    let module = Module::new(&engine, &std::fs::read(&a[1])?[..])?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(memory)
        .table_elements(1_000_000)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|l| l);
    store.set_fuel(fuel)?;
    let instance = Linker::new(&engine).instantiate_and_start(&mut store, &module)?;
    instance
        .get_typed_func::<u32, u32>(&store, "telora_gc_heap_limit")?
        .call(&mut store, heap)?;
    instance
        .get_typed_func::<u32, u32>(&store, "telora_gc_interval")?
        .call(&mut store, interval)?;
    let now = std::time::Instant::now();
    let result = instance
        .get_typed_func::<(), i32>(&store, "create-service")?
        .call(&mut store, ());
    eprintln!(
        "initialize={result:?} seconds={} fuel={} memory={}",
        now.elapsed().as_secs_f64(),
        fuel - store.get_fuel()?,
        instance
            .get_memory(&store, "memory")
            .unwrap()
            .data_size(&store)
    );
    store.set_fuel(10_000_000)?;
    for name in [
        "telora_heap_bytes",
        "telora_heap_committed",
        "telora_gc_collections",
    ] {
        let f = instance.get_typed_func::<(), u32>(&store, name)?;
        eprintln!("{name}={}", f.call(&mut store, ())?);
    }
    let f = instance.get_typed_func::<(), u64>(&store, "telora_gc_allocated_total")?;
    eprintln!("allocated={}", f.call(&mut store, ())?);
    result?;
    Ok(())
}
