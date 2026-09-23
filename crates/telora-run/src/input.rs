//! Inject source text through Guest parsing; no Host parser or language values.
use crate::fuel_quota::FuelQuota;
use crate::{artifact::ModuleData, engine::Guest};
use anyhow::{Result, ensure};

pub struct SourceInput {
    pub name: String,
    pub data: Vec<u8>,
    /// 1 = JSON, 2 = YAML, 3 = TOML.
    pub format: u32,
}

impl Guest {
    pub fn inject_module(&mut self, module: &ModuleData, quota: &mut FuelQuota) -> Result<()> {
        let ptr = self.transfer(module.source.name.as_bytes())?;
        let register = self
            .instance
            .get_typed_func::<(u32, u32, u32), u32>(&mut self.store, "telora_register_source")?;
        let ok = quota.call(
            &mut self.store,
            register,
            (
                module.source.id,
                ptr,
                u32::try_from(module.source.name.len())?,
            ),
        )?;
        self.free(ptr, module.source.name.len(), 1)?;
        ensure!(ok == 1, "data source identity conflict");
        let ptr = self.transfer(module.text.as_bytes())?;
        let parse = self
            .instance
            .get_typed_func::<(u32, u32, u32, u32), u32>(&mut self.store, "telora_parse_data")?;
        let packet = quota.call(
            &mut self.store,
            parse,
            (
                ptr,
                u32::try_from(module.text.len())?,
                module.format,
                module.source.id,
            ),
        )?;
        self.free(ptr, module.text.len(), 1)?;
        let error = self.word(packet + 12)?;
        if error != 0 {
            let ptr = self.word(error + 8)?;
            let len = self.word(error + 12)?;
            let text = std::str::from_utf8(self.bytes(self.address(ptr, len)?, len)?)?;
            let diagnostics = serde_json::from_str(&format!("[{text}]"))?;
            return Err(crate::InitializationError { diagnostics }.into());
        }
        let materialize = self
            .instance
            .get_typed_func::<(u32, u32), u32>(&mut self.store, "telora_materialize_data")?;
        let value = quota.call(&mut self.store, materialize, (packet, 0))?;
        ensure!(value != 0, "data materialization failed");
        let inject = self
            .instance
            .get_typed_func::<(u32, u32), u32>(&mut self.store, "telora_inject_data")?;
        ensure!(
            quota.call(&mut self.store, inject, (module.symbol, value))? == 1,
            "data injection failed"
        );
        Ok(())
    }
    pub fn sources(&mut self, quota: &mut FuelQuota) -> Result<Vec<(u32, String)>> {
        let count = quota.call(&mut self.store, self.exports.count, ())?;
        let result = self.alloc(12, 4)?;
        let mut names = vec![];
        for index in 0..count {
            quota.call(&mut self.store, self.exports.name, (index, result))?;
            let id = self.raw_word(result)?;
            let ptr = self.raw_word(result + 4)?;
            let len = self.raw_word(result + 8)?;
            names.push((id, std::str::from_utf8(self.bytes(ptr, len)?)?.to_owned()));
        }
        self.free(result, 12, 4)?;
        Ok(names)
    }
    pub fn inject_named_sources(
        &mut self,
        names: &[(u32, String)],
        sources: &[SourceInput],
        quota: &mut FuelQuota,
    ) -> Result<()> {
        ensure!(
            names.len() == sources.len(),
            "service source count mismatch: expected {:?}",
            names.iter().map(|(_, name)| name).collect::<Vec<_>>()
        );
        for ((id, name), source) in names.iter().zip(sources) {
            ensure!(
                name == &source.name,
                "expected source {name:?}, got {:?}",
                source.name
            );
            ensure!((1..=3).contains(&source.format), "invalid source format");
            let ptr = self.transfer(&source.data)?;
            quota.call(
                &mut self.store,
                self.exports.set,
                (*id, ptr, u32::try_from(source.data.len())?, source.format),
            )?;
            self.free(ptr, source.data.len(), 1)?;
        }
        Ok(())
    }
}
