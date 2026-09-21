//! Host passes exact roots; the Rust Wasm RT performs traversal and copying.
use crate::{abi, session::Session, transport::Value};

#[derive(Debug)]
pub struct CollectionStats {
    pub heap_before: u32,
    pub heap_after: u32,
    pub memory_bytes: usize,
}

#[derive(Debug)]
pub struct InitializationStats {
    pub heap_before: u32,
    pub heap_after: u32,
    pub demand_roots: u32,
    pub memory_before: u32,
    pub memory_high_water: u32,
}

impl Session {
    pub fn initialization_stats(&mut self) -> Result<InitializationStats, String> {
        let metric = self
            .instance
            .get_typed_func::<u32, u32>(&self.store, "telora_initialization_stat")
            .map_err(|e| e.to_string())?;
        Ok(InitializationStats {
            heap_before: metric.call(&mut self.store, 0).map_err(|e| e.to_string())?,
            heap_after: metric.call(&mut self.store, 1).map_err(|e| e.to_string())?,
            demand_roots: metric.call(&mut self.store, 2).map_err(|e| e.to_string())?,
            memory_before: metric.call(&mut self.store, 3).map_err(|e| e.to_string())?,
            memory_high_water: metric.call(&mut self.store, 4).map_err(|e| e.to_string())?,
        })
    }

    pub fn collect_work(
        &mut self,
        roots: &[Value],
    ) -> Result<(Vec<Value>, CollectionStats), String> {
        for &root in roots {
            self.expect_value(root, root.ty)?;
        }
        let heap_end = self
            .instance
            .get_typed_func::<(), i32>(&self.store, "telora_heap_bytes")
            .map_err(|e| e.to_string())?;
        let before = heap_end
            .call(&mut self.store, ())
            .map_err(|e| e.to_string())? as u32;
        let pointers = self.allocate(
            roots
                .len()
                .checked_mul(8)
                .ok_or("Wasm: root size overflow")?,
        )?;
        for (index, root) in roots.iter().enumerate() {
            self.write(pointers as usize + index * 8, &root.pointer.to_le_bytes())?;
            self.write(pointers as usize + index * 8 + 4, &root.ty.to_le_bytes())?;
        }
        let collect = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_collect")
            .map_err(|e| e.to_string())?;
        let relocated = collect
            .call(&mut self.store, (pointers as i32, roots.len() as i32))
            .map_err(|e| e.to_string())? as u32;
        let values = roots
            .iter()
            .enumerate()
            .map(|(index, root)| {
                Ok(Value {
                    pointer: self.output().word(relocated as u64 + index as u64 * 4)?,
                    ty: root.ty,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let after = heap_end
            .call(&mut self.store, ())
            .map_err(|e| e.to_string())? as u32;
        let retained = self
            .instance
            .get_typed_func::<i32, i32>(&self.store, "telora_source_retained")
            .map_err(|e| e.to_string())?;
        let mut live = std::collections::BTreeSet::new();
        for source in &self.manifest.sources {
            if retained
                .call(&mut self.store, source.id as i32)
                .map_err(|e| e.to_string())?
                != 0
            {
                live.insert(source.id);
            }
        }
        self.manifest
            .sources
            .retain(|source| live.contains(&source.id));
        self.registered_sources = self.manifest.sources.len();
        self.emitted_debug.set(
            self.output()
                .word(abi::table_address(abi::DEBUG_EVENTS) as u64 + 4)?,
        );
        Ok((
            values,
            CollectionStats {
                heap_before: before,
                heap_after: after,
                memory_bytes: self.memory.data(&self.store).len(),
            },
        ))
    }
}
