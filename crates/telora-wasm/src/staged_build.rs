//! Continuous build-time evaluation and publication for an ordinary function.
use crate::{
    session::Session,
    specialization::RuntimePlan,
    transform_service::{buffers, export_snapshot_mode, import_snapshot},
};
use telora_core::mir::SealedExecutable;
use telora_wasm_shared::snapshot_artifact::{GlobalValue, Snapshot};

pub struct StagedBuild {
    bytes: Vec<u8>,
    plan: RuntimePlan,
}
impl StagedBuild {
    pub fn compile(executable: &SealedExecutable<'_>) -> Result<Self, String> {
        let linked = crate::codegen::compile_staged_build(executable)?;
        Ok(Self {
            bytes: linked.bytes,
            plan: linked.runtime_plan,
        })
    }
    /// P1 and compiler evidence cannot be supplied by another build.
    pub fn publish(self, fuel: u64, memory: usize) -> Result<Vec<u8>, String> {
        let mut session =
            Session::load_with_limits_observed(&self.bytes, fuel, memory, &mut |_, _| {})?;
        if session.module.imports().next().is_some() {
            return Err("staged build rejects Host imports".into());
        }
        if !session.manifest.data_modules.is_empty() {
            return Err("staged function API does not yet support data modules".into());
        }
        if session.manifest.types[session.manifest.entry_type as usize].kind
            != crate::artifact::Kind::Function
        {
            return Err("staged program must export a function".into());
        }
        scope(&mut session, true)?;
        session.initialize()?;
        let value = session.entry()?;
        scope(&mut session, false)?;
        root(&mut session, value)?;
        let information = take_information(&mut session)?;
        let globals = capture_globals(&session)?;
        let (guest, dependencies) = export_snapshot_mode(&mut session, true)?;
        let (bytes, _) = crate::specialization::runtime(
            &self.bytes,
            &self.plan,
            &dependencies.ok_or("missing staged dependencies")?,
        )?;
        let bytes = crate::publication::attach_snapshot(&bytes, &Snapshot { guest, globals })?;
        crate::publication::attach_information(&bytes, &information)
    }
}

/// Load only the snapshot carried by the same compiled artifact.
pub fn load(bytes: &[u8], fuel: u64, memory: usize) -> Result<Session, String> {
    let snapshots: Vec<_> = wasmparser::Parser::new(0)
        .parse_all(bytes)
        .filter_map(|part| match part {
            Ok(wasmparser::Payload::CustomSection(section))
                if section.name() == telora_wasm_shared::snapshot_artifact::SECTION =>
            {
                Some(Ok(section.data()))
            }
            Err(error) => Some(Err(error.to_string())),
            _ => None,
        })
        .collect::<Result<_, _>>()?;
    if snapshots.len() != 1 {
        return Err("expected one staged program snapshot".into());
    }
    let snapshot = telora_wasm_shared::snapshot_artifact::decode(snapshots[0])?;
    let mut session = Session::load_with_limits_observed(bytes, fuel, memory, &mut |_, _| {})?;
    root(&mut session, 0)?;
    import_snapshot(&mut session, &snapshot.guest)?;
    for (name, value) in snapshot.globals {
        let value = match value {
            GlobalValue::I32(v) => wasmi::Val::I32(v),
            GlobalValue::I64(v) => wasmi::Val::I64(v),
            GlobalValue::F32(v) => wasmi::Val::F32(f32::from_bits(v).into()),
            GlobalValue::F64(v) => wasmi::Val::F64(f64::from_bits(v).into()),
        };
        session
            .instance
            .get_global(&session.store, &name)
            .ok_or("missing staged reset global")?
            .set(&mut session.store, value)
            .map_err(|e| e.to_string())?;
    }
    session.entry_export = "telora_program_entry";
    session.registered_sources = session.manifest.sources.len();
    session.initialization_quota = None;
    Ok(session)
}
pub(crate) fn scope(session: &mut Session, active: bool) -> Result<(), String> {
    let call = session
        .instance
        .get_typed_func::<u32, ()>(&session.store, "telora_build_time_scope")
        .map_err(|e| e.to_string())?;
    session.guest_call(call, active as u32)
}
fn root(session: &mut Session, value: u32) -> Result<(), String> {
    let call = session
        .instance
        .get_typed_func::<(u32, u32), ()>(&session.store, "telora_program_root")
        .map_err(|e| e.to_string())?;
    session.guest_call(call, (session.manifest.entry_type, value))
}
fn capture_globals(session: &Session) -> Result<Vec<(String, GlobalValue)>, String> {
    session
        .instance
        .exports(&session.store)
        .filter_map(|export| {
            let name = export.name().to_owned();
            if !name.starts_with("telora_reset_global_") {
                return None;
            }
            let global = export.into_global()?;
            if !global.ty(&session.store).mutability().is_mut() {
                return None;
            }
            Some(match global.get(&session.store) {
                wasmi::Val::I32(v) => Ok((name, GlobalValue::I32(v))),
                wasmi::Val::I64(v) => Ok((name, GlobalValue::I64(v))),
                wasmi::Val::F32(v) => Ok((name, GlobalValue::F32(v.to_bits()))),
                wasmi::Val::F64(v) => Ok((name, GlobalValue::F64(v.to_bits()))),
                _ => Err("staged reset globals must be scalar".into()),
            })
        })
        .collect()
}
pub(crate) fn take_information(
    session: &mut Session,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>, String> {
    let export = session
        .instance
        .get_typed_func::<u32, ()>(&session.store, "telora_build_information_export")
        .map_err(|e| e.to_string())?;
    let result = buffers::alloc(session, 12, 4)?;
    session.guest_call(export, result)?;
    let [pointer, length, _] = buffers::words(session, result)?;
    let bytes = buffers::bytes(session, pointer, length)?;
    buffers::free(session, pointer, length, 1)?;
    buffers::free(session, result, 12, 4)?;
    fn word(input: &mut &[u8]) -> Result<usize, String> {
        let (word, rest) = input
            .split_first_chunk::<4>()
            .ok_or("truncated build information")?;
        *input = rest;
        Ok(u32::from_le_bytes(*word) as usize)
    }
    fn field<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], String> {
        let size = word(input)?;
        let value = input
            .get(..size)
            .ok_or("truncated build information field")?;
        *input = &input[size..];
        Ok(value)
    }
    let mut input = bytes.as_slice();
    let count = word(&mut input)?;
    let mut metadata = std::collections::BTreeMap::new();
    for _ in 0..count {
        let name = std::str::from_utf8(field(&mut input)?)
            .map_err(|e| e.to_string())?
            .to_owned();
        let value = serde_json::from_slice(field(&mut input)?).map_err(|e| e.to_string())?;
        if name.is_empty() || metadata.insert(name, value).is_some() {
            return Err("invalid build information name".into());
        }
    }
    if !input.is_empty() {
        return Err("trailing build information".into());
    }
    Ok(metadata)
}
