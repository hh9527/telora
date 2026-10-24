//! Derive B from this build's A.result without renumbering types or table slots.
//! Exact closure roots come from the runtime's closed-layout traversal; code
//! edges come from linker relocations, never from guessing integer bit patterns.
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{CodeSection, Function, Instruction as I};

pub(crate) struct RuntimePlan {
    pub(crate) function_base: u32,
    pub(crate) table_base: u32,
    pub(crate) count: u32,
    pub(crate) dependencies: Vec<BTreeSet<u32>>,
    pub(crate) heap_address: u32,
    pub(crate) runtime_roots: Vec<u32>,
    pub(crate) demands: Vec<Demand>,
}

pub(crate) struct Demand {
    pub index: u32,
    pub function: u32,
    pub address: u32,
    pub width: u32,
}

pub(crate) struct LinkedProgram {
    pub bytes: Vec<u8>,
    pub runtime_plan: RuntimePlan,
}

pub(crate) struct RuntimeDependencies {
    pub functions: Vec<u32>,
    pub ready_demands: BTreeSet<u32>,
}

#[derive(Serialize)]
pub struct Report {
    schema: &'static str,
    pub generated_functions: usize,
    pub retained_functions: usize,
    pub prepared_getters: usize,
    pub removed_body_bytes: usize,
    pub stubbed_functions: usize,
    pub original_code_bytes: usize,
    pub runtime_code_bytes: usize,
    pub snapshot_bytes: usize,
}

// Same caller-owned output contract as a Ready demand's ordinary epilogue.
// This getter allocates nothing and cannot reach a GC safepoint.
fn getter(address: u32, width: u32, heap_address: u32) -> Function {
    let mut code = Function::new([(1, wasm_encoder::ValType::I32)]);
    let memory = wasm_encoder::MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    };
    for instruction in [
        I::I32Const(address as i32),
        I::I32Load(memory),
        I::LocalSet(3),
        I::LocalGet(2),
        I::If(wasm_encoder::BlockType::Result(wasm_encoder::ValType::I32)),
        I::LocalGet(2),
        I::Call(heap_address),
        I::LocalGet(3),
        I::Call(heap_address),
        I::I32Const(width as i32),
        I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        },
        I::LocalGet(2),
        I::Else,
        I::LocalGet(3),
        I::End,
        I::End,
    ] {
        code.instruction(&instruction);
    }
    code
}

pub(crate) fn runtime(
    bytes: &[u8],
    plan: &RuntimePlan,
    dependencies: &RuntimeDependencies,
) -> Result<(Vec<u8>, Report), String> {
    let mut bodies = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.map_err(|e| e.to_string())? {
            bodies.push(body.range());
        }
    }
    if plan.dependencies.len() != plan.count as usize {
        return Err("invalid specialization plan".into());
    }
    let getters: BTreeMap<_, _> = plan
        .demands
        .iter()
        .filter(|demand| dependencies.ready_demands.contains(&demand.index))
        .map(|demand| (demand.function, (demand.address, demand.width)))
        .collect();
    let mut pending = plan.runtime_roots.clone();
    for &function in &dependencies.functions {
        // RT functions keep their original bodies in this first implementation.
        if let Some(ordinal) = function.checked_sub(plan.table_base) {
            if ordinal >= plan.count {
                return Err("invalid snapshot function root".into());
            }
            pending.push(ordinal);
        }
    }
    let mut retained = BTreeSet::new();
    while let Some(function) = pending.pop() {
        if function >= plan.count {
            return Err("invalid code dependency".into());
        }
        if retained.insert(function) && !getters.contains_key(&function) {
            pending.extend(plan.dependencies[function as usize].iter().copied());
        }
    }
    if retained.is_empty() {
        return Err("runtime publication has no generated function roots".into());
    }
    let mut report = Report {
        schema: "telora.runtime-specialization/v1",
        generated_functions: plan.count as usize,
        retained_functions: retained.len(),
        prepared_getters: 0,
        removed_body_bytes: 0,
        stubbed_functions: plan.count as usize - retained.len(),
        original_code_bytes: bodies
            .iter()
            .map(|range| (range.end - range.start) as usize)
            .sum(),
        runtime_code_bytes: 0,
        snapshot_bytes: 0,
    };
    let mut code = CodeSection::new();
    for (index, range) in bodies.iter().enumerate() {
        let ordinal = (index as u32).checked_sub(plan.function_base);
        if let Some(ordinal) = ordinal.filter(|&i| i < plan.count) {
            if !retained.contains(&ordinal) {
                let mut stub = Function::new([]);
                stub.instruction(&I::Unreachable).instruction(&I::End);
                code.function(&stub);
                continue;
            }
            if let Some(&(address, width)) = getters.get(&ordinal) {
                let body = getter(address, width, plan.heap_address);
                report.prepared_getters += 1;
                code.function(&body);
                continue;
            }
        }
        code.raw(&bytes[range.start as usize..range.end as usize]);
    }
    let mut module = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| e.to_string())?;
        if let wasmparser::Payload::CustomSection(section) = &payload {
            if matches!(section.name(), "name") {
                continue;
            }
        }
        if let Some((id, range)) = payload.as_section() {
            if id == 10 {
                module.section(&code);
            } else {
                module.section(&wasm_encoder::RawSection {
                    id,
                    data: &bytes[range.start as usize..range.end as usize],
                });
            }
        }
    }
    let result = module.finish();
    for payload in wasmparser::Parser::new(0).parse_all(&result) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.map_err(|e| e.to_string())? {
            report.runtime_code_bytes += (body.range().end - body.range().start) as usize;
        }
    }
    report.removed_body_bytes = report
        .original_code_bytes
        .saturating_sub(report.runtime_code_bytes);
    wasmparser::Validator::new()
        .validate_all(&result)
        .map_err(|e| format!("invalid specialized module: {e}"))?;
    Ok((result, report))
}
