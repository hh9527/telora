//! Host passes exact roots; the Rust Wasm RT performs traversal and copying.
use crate::{abi, artifact::Kind, session::Session, transport::Value};

#[derive(Debug)]
pub struct CollectionStats {
    pub heap_before: u32,
    pub heap_after: u32,
    pub memory_bytes: usize,
}

impl Session {
    pub(crate) fn prepare_collection(&mut self) -> Result<(), String> {
        if self.trace_types != 0 {
            return Ok(());
        }
        let mut image = vec![0u8; self.manifest.types.len() * 20];
        for (index, ty) in self.manifest.types.iter().enumerate() {
            let kind: u32 = match ty.kind {
                Kind::Int | Kind::Float | Kind::Bool | Kind::Unit | Kind::Metadata => 0,
                Kind::String => 1,
                Kind::Bytes => 2,
                Kind::Record | Kind::Tuple => 3,
                Kind::Array => 4,
                Kind::Dict => 5,
                Kind::Enum | Kind::Option | Kind::Value => 6,
                Kind::Dyn => 7,
                Kind::Function => 8,
                Kind::Unsupported if ty.resource_table.is_some() => 9,
                Kind::Newtype => 10,
                Kind::Unsupported => u32::MAX,
            };
            let variants = u32::try_from(image.len()).map_err(|_| "Wasm: trace image overflow")?;
            for (offset, word) in [
                kind,
                ty.bytes,
                ty.resource_table.unwrap_or(u32::MAX),
                ty.variants.len() as u32,
                variants,
            ]
            .into_iter()
            .enumerate()
            {
                image[index * 20 + offset * 4..index * 20 + offset * 4 + 4]
                    .copy_from_slice(&word.to_le_bytes());
            }
            for variant in &ty.variants {
                image.extend_from_slice(&variant.ty.unwrap_or(u32::MAX).to_le_bytes());
                image.extend_from_slice(&u32::from(variant.boxed).to_le_bytes());
            }
        }
        let pointer = self.allocate(image.len())?;
        self.write(pointer as usize, &image)?;
        self.trace_types = pointer;
        Ok(())
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
                .checked_mul(4)
                .ok_or("Wasm: root size overflow")?,
        )?;
        for (index, root) in roots.iter().enumerate() {
            self.write(pointers as usize + index * 4, &root.pointer.to_le_bytes())?;
        }
        let collect = self
            .instance
            .get_typed_func::<(i32, i32, i32), i32>(&self.store, "telora_collect")
            .map_err(|e| e.to_string())?;
        let relocated = collect
            .call(
                &mut self.store,
                (self.trace_types as i32, pointers as i32, roots.len() as i32),
            )
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
        let retained = self.instance
            .get_typed_func::<i32, i32>(&self.store, "telora_source_retained")
            .map_err(|e| e.to_string())?;
        let mut live = std::collections::BTreeSet::new();
        for source in &self.manifest.sources {
            if retained.call(&mut self.store, source.id as i32).map_err(|e| e.to_string())? != 0 {
                live.insert(source.id);
            }
        }
        self.manifest.sources.retain(|source| live.contains(&source.id));
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
