//! Workspace-wide compiler and runtime defaults. JSON quantities use CLI units.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CompilerOptions {
    pub max_type_depth: usize,
    pub max_tuple_items: usize,
    pub max_type_arguments: usize,
}

impl Default for CompilerOptions {
    fn default() -> Self {
        Self { max_type_depth: 256, max_tuple_items: 1024, max_type_arguments: 4096 }
    }
}

impl CompilerOptions {
    pub fn validate(self) -> Result<(), String> {
        for (name, value) in [
            ("maxTypeDepth", self.max_type_depth),
            ("maxTupleItems", self.max_tuple_items),
            ("maxTypeArguments", self.max_type_arguments),
        ] {
            if value == 0 { return Err(format!("compiler.{name} must be a positive integer")); }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeOptions {
    /// Millions of Wasm fuel units.
    pub fuel: u64,
    /// Mebibytes of Wasm linear memory (each is sixteen 64 KiB pages).
    pub memory_limit: u64,
}

impl Default for RuntimeOptions {
    fn default() -> Self { Self { fuel: 100, memory_limit: 1024 } }
}

impl RuntimeOptions {
    pub fn limits(self) -> Result<(u64, usize), String> {
        let fuel = self.fuel.checked_mul(1_000_000).filter(|&n| n != 0)
            .ok_or("runtime.fuel must be a positive integer whose value in fuel units fits u64")?;
        let memory = self.memory_limit.checked_mul(1 << 20)
            .and_then(|n| usize::try_from(n).ok()).filter(|&n| n != 0)
            .ok_or("runtime.memoryLimit must be a positive integer whose value in bytes fits usize")?;
        Ok((fuel, memory))
    }
}
