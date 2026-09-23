//! Fixed Guest ABI shared by engines; handles are bound once per instance.
use crate::backend::{
    self,
    runtime::{Instance, Memory, Module, Store, StoreLimits, TypedFunc},
};
use anyhow::{Result, ensure};

pub(crate) struct Exports {
    pub alloc: TypedFunc<(u32, u32), u32>,
    pub free: TypedFunc<(u32, u32, u32), ()>,
    pub count: TypedFunc<(), u32>,
    pub name: TypedFunc<(u32, u32), ()>,
    pub set: TypedFunc<(u32, u32, u32, u32), ()>,
    pub create: TypedFunc<(), i32>,
    pub run: TypedFunc<(u32, u32, u32, u32, u32), ()>,
    pub reset: TypedFunc<(), ()>,
    pub diagnostics: TypedFunc<(u32, u32, u32), ()>,
    pub heap_bytes: TypedFunc<(), u32>,
    pub initialization_stat: TypedFunc<u32, u32>,
    pub snapshot_export: TypedFunc<u32, ()>,
    pub snapshot_import: TypedFunc<(u32, u32), ()>,
}

pub(crate) struct Guest {
    pub module: Module,
    pub store: Store<StoreLimits>,
    pub instance: Instance,
    pub memory: Memory,
    pub exports: Exports,
}

pub(crate) fn limits(memory: usize) -> StoreLimits {
    backend::runtime::StoreLimitsBuilder::new()
        .memory_size(memory)
        .table_elements(1_000_000)
        .trap_on_grow_failure(true)
        .build()
}

impl Guest {
    pub fn instantiate(module: Module, fuel: u64, memory_limit: usize) -> Result<Self> {
        let mut store = Store::new(module.engine(), limits(memory_limit));
        store.limiter(|limits| limits);
        store.set_fuel(fuel)?;
        let instance = backend::instantiate(&module, &mut store)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow::anyhow!("missing memory"))?;
        let exports = Exports {
            alloc: instance.get_typed_func(&mut store, "mem-alloc")?,
            free: instance.get_typed_func(&mut store, "mem-free")?,
            count: instance.get_typed_func(&mut store, "get-data-source-count")?,
            name: instance.get_typed_func(&mut store, "get-data-source-name")?,
            set: instance.get_typed_func(&mut store, "set-data-source")?,
            create: instance.get_typed_func(&mut store, "create-service")?,
            run: instance.get_typed_func(&mut store, "run-service")?,
            reset: instance.get_typed_func(&mut store, "reset-service")?,
            diagnostics: instance.get_typed_func(&mut store, "get-service-diagnostics")?,
            heap_bytes: instance.get_typed_func(&mut store, "telora_heap_bytes")?,
            initialization_stat: instance
                .get_typed_func(&mut store, "telora_initialization_stat")?,
            snapshot_export: instance.get_typed_func(&mut store, "telora_snapshot_export")?,
            snapshot_import: instance.get_typed_func(&mut store, "telora_snapshot_import")?,
        };
        Ok(Self {
            module,
            store,
            instance,
            memory,
            exports,
        })
    }
    pub fn alloc(&mut self, bytes: usize, align: u32) -> Result<u32> {
        let ptr = self
            .exports
            .alloc
            .call(&mut self.store, (u32::try_from(bytes)?, align))?;
        ensure!(ptr != 0 && ptr % align == 0, "invalid Guest allocation");
        Ok(ptr)
    }
    pub fn free(&mut self, ptr: u32, cap: usize, align: u32) -> Result<()> {
        self.exports
            .free
            .call(&mut self.store, (ptr, u32::try_from(cap)?, align))?;
        Ok(())
    }
    pub fn transfer(&mut self, bytes: &[u8]) -> Result<u32> {
        let ptr = self.alloc(bytes.len(), 1)?;
        self.memory.write(&mut self.store, ptr as usize, bytes)?;
        Ok(ptr)
    }
    pub fn bytes(&self, ptr: u32, len: u32) -> Result<&[u8]> {
        let end = ptr
            .checked_add(len)
            .ok_or_else(|| anyhow::anyhow!("Guest range overflow"))?;
        self.memory
            .data(&self.store)
            .get(ptr as usize..end as usize)
            .ok_or_else(|| anyhow::anyhow!("Guest range out of bounds"))
    }
    pub fn raw_word(&self, ptr: u32) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(ptr, 4)?.try_into()?))
    }
    pub fn address(&self, reference: u32, len: u32) -> Result<u32> {
        use telora_wasm_shared::abi::{WORDS_ORIGIN, WORDS_VIEW};
        let origin = self.raw_word(WORDS_ORIGIN)?;
        if reference < origin {
            return Ok(reference);
        }
        let offset = reference - origin;
        ensure!(
            u64::from(offset) + u64::from(len) <= u64::from(self.raw_word(WORDS_VIEW + 4)?),
            "language heap range out of bounds"
        );
        self.raw_word(WORDS_VIEW)?
            .checked_add(offset)
            .ok_or_else(|| anyhow::anyhow!("heap address overflow"))
    }
    pub fn word(&self, reference: u32) -> Result<u32> {
        self.raw_word(self.address(reference, 4)?)
    }
    pub fn response(&mut self, record: u32) -> Result<Vec<u8>> {
        let ptr = self.raw_word(record)?;
        let len = self.raw_word(record + 4)?;
        let cap = self.raw_word(record + 8)?;
        ensure!(ptr != 0 && len <= cap, "invalid Guest output buffer");
        self.bytes(ptr, cap)?;
        let bytes = self.bytes(ptr, len)?.to_vec();
        self.free(ptr, cap as usize, 1)?;
        self.free(record, 12, 4)?;
        Ok(bytes)
    }
    pub fn diagnostics(&mut self) -> Result<Vec<serde_json::Value>> {
        let record = self.alloc(12, 4)?;
        self.exports
            .diagnostics
            .call(&mut self.store, (1, 0, record))?;
        Ok(serde_json::from_slice(&self.response(record)?)?)
    }
    pub fn request_with_quota(
        &mut self,
        bytes: &[u8],
        quota: &mut crate::fuel_quota::FuelQuota,
    ) -> Result<Vec<u8>> {
        let ptr = self.transfer(bytes)?;
        let result = self.alloc(12, 4)?;
        quota.call(
            &mut self.store,
            self.exports.run,
            (ptr, u32::try_from(bytes.len())?, 1, 0, result),
        )?;
        self.free(ptr, bytes.len(), 1)?;
        self.response(result)
    }

    pub fn export_snapshot(&mut self) -> Result<Vec<u8>> {
        let result = self.alloc(12, 4)?;
        self.exports.snapshot_export.call(&mut self.store, result)?;
        let pointer = self.raw_word(result)?;
        let length = self.raw_word(result + 4)?;
        let bytes = self.bytes(pointer, length)?.to_vec();
        self.free(pointer, length as usize, 1)?;
        self.free(result, 12, 4)?;
        Ok(bytes)
    }

    pub fn import_snapshot(&mut self, snapshot: &[u8]) -> Result<()> {
        let pointer = self.transfer(snapshot)?;
        self.exports
            .snapshot_import
            .call(&mut self.store, (pointer, u32::try_from(snapshot.len())?))?;
        self.free(pointer, snapshot.len(), 1)
    }
}
