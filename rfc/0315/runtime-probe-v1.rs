//! Raw Guest phase measurements, shared by ABI 37 baseline and ABI 38 candidate.
use std::{error::Error, time::Instant};
use wasmi::{
    Config, CustomFuelCosts, Engine, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const FUEL: u64 = 250_000_000_000;
struct Guest {
    store: Store<StoreLimits>,
    instance: Instance,
    memory: Memory,
}
impl Guest {
    fn new(engine: &Engine, module: &Module) -> Result<Self> {
        let mut store = Store::new(
            engine,
            StoreLimitsBuilder::new()
                .memory_size(64 * 1024 * 1024)
                .trap_on_grow_failure(true)
                .build(),
        );
        store.limiter(|limits| limits);
        store.set_fuel(FUEL)?;
        let instance = Linker::new(engine).instantiate_and_start(&mut store, module)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("missing memory")?;
        Ok(Self {
            store,
            instance,
            memory,
        })
    }
    fn alloc(&mut self, bytes: u32) -> Result<u32> {
        Ok(self
            .instance
            .get_typed_func::<(u32, u32), u32>(&self.store, "mem-alloc")?
            .call(&mut self.store, (bytes, 1))?)
    }
    fn free(&mut self, at: u32, capacity: u32) -> Result<()> {
        Ok(self
            .instance
            .get_typed_func::<(u32, u32, u32), ()>(&self.store, "mem-free")?
            .call(&mut self.store, (at, capacity, 1))?)
    }
    fn word(&self, at: u32) -> u32 {
        u32::from_le_bytes(
            self.memory.data(&self.store)[at as usize..at as usize + 4]
                .try_into()
                .unwrap(),
        )
    }
    fn metric(&mut self, name: &str) -> Result<u32> {
        Ok(self
            .instance
            .get_typed_func::<(), u32>(&self.store, name)?
            .call(&mut self.store, ())?)
    }
    fn measure<T>(
        &mut self,
        phase: &str,
        action: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.store.set_fuel(FUEL)?;
        let start = Instant::now();
        let value = action(self)?;
        let seconds = start.elapsed().as_secs_f64();
        let fuel = FUEL - self.store.get_fuel()?;
        let managed = self.metric("telora_heap_bytes")?;
        let committed = self.metric("telora_heap_committed")?;
        let gc = self.metric("telora_gc_collections")?;
        let allocated = self
            .instance
            .get_typed_func::<(), u64>(&self.store, "telora_gc_allocated_total")?
            .call(&mut self.store, ())?;
        println!(
            "{{\"phase\":\"{phase}\",\"seconds\":{seconds},\"fuel\":{fuel},\"managed\":{managed},\"committed\":{committed},\"collections\":{gc},\"allocated_total\":{allocated},\"memory\":{}}}",
            self.memory.data_size(&self.store)
        );
        Ok(value)
    }
    fn snapshot(&mut self) -> Result<Vec<u8>> {
        let result = self.alloc(8)?;
        self.instance
            .get_typed_func::<u32, ()>(&self.store, "telora_snapshot_export")?
            .call(&mut self.store, result)?;
        let pointer = self.word(result);
        let length = self.word(result + 4);
        let image = self.memory.data(&self.store)
            [pointer as usize..pointer as usize + length as usize]
            .to_vec();
        self.free(pointer, length)?;
        self.free(result, 8)?;
        Ok(image)
    }
    fn restore(&mut self, image: &[u8]) -> Result<()> {
        let length = u32::try_from(image.len())?;
        let pointer = self.alloc(length)?;
        self.memory
            .write(&mut self.store, pointer as usize, image)?;
        self.instance
            .get_typed_func::<(u32, u32), ()>(&self.store, "telora_snapshot_import")?
            .call(&mut self.store, (pointer, length))?;
        self.free(pointer, length)
    }
    fn request(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        let length = u32::try_from(input.len())?;
        let pointer = self.alloc(length)?;
        self.memory
            .write(&mut self.store, pointer as usize, input)?;
        let result = self.alloc(12)?;
        self.instance
            .get_typed_func::<(u32, u32, u32, u32, u32), ()>(&self.store, "run-service")?
            .call(&mut self.store, (pointer, length, 1, 0, result))?;
        let at = self.word(result);
        let size = self.word(result + 4);
        let capacity = self.word(result + 8);
        let output =
            self.memory.data(&self.store)[at as usize..at as usize + size as usize].to_vec();
        self.free(at, capacity)?;
        self.free(result, 12)?;
        self.free(pointer, length)?;
        self.instance
            .get_typed_func::<(), ()>(&self.store, "reset-service")?
            .call(&mut self.store, ())?;
        Ok(output)
    }
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mut config = Config::default();
    config.consume_fuel(true).fuel_cost(CustomFuelCosts {
        bytes_copied_per_fuel: 64,
        fuel_per_bytes_translated: 0,
        fuel_per_bytes_validated: 0,
    });
    let engine = Engine::new(&config);
    let module = Module::new(&engine, &std::fs::read(&args[1])?[..])?;
    let mut source = Guest::new(&engine, &module)?;
    source.measure("initialize", |guest| {
        let status = guest
            .instance
            .get_typed_func::<(), i32>(&guest.store, "create-service")?
            .call(&mut guest.store, ())?;
        if status != 0 {
            return Err(format!("initialization status {status}").into());
        }
        Ok(())
    })?;
    let image = source.measure("pack", |guest| guest.snapshot())?;
    println!("{{\"image_bytes\":{}}}", image.len());
    let mut restored = Guest::new(&engine, &module)?;
    restored.measure("restore", |guest| guest.restore(&image))?;
    let mut outputs = Vec::new();
    for request in std::fs::read_to_string(&args[2])?
        .lines()
        .filter(|line| !line.is_empty())
    {
        let original = source.measure("request", |guest| guest.request(request.as_bytes()))?;
        let copy = restored.measure("restored_request", |guest| {
            guest.request(request.as_bytes())
        })?;
        if original != copy {
            return Err("restored output differs".into());
        }
        outputs.extend_from_slice(&original);
        outputs.push(b'\n');
    }
    std::fs::write(&args[3], outputs)?;
    Ok(())
}
