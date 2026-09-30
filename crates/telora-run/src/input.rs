//! Inject source text through Guest parsing; no Host parser or language values.
use crate::engine::Guest;
use crate::fuel_quota::FuelQuota;
use anyhow::{Result, ensure};

pub struct SourceInput {
    pub name: String,
    pub data: Vec<u8>,
    /// 1 = JSON, 2 = YAML, 3 = TOML.
    pub format: u32,
}

impl Guest {
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
