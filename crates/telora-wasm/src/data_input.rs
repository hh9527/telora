//! Source-backed data transport. Materialize once, then inject before initialization.
use crate::{
    abi::*,
    artifact::{Kind, Manifest},
    plan::Plan,
    session::Session,
};
use telora_core::data_plan::{DataNodeId, DataPlanNodeKind, DataScalar, ValidatedDataPlan};
use wasm_encoder::{BlockType, Function, Instruction as I};

pub(crate) fn injector(plan: &Plan, manifest: &Manifest) -> Function {
    let mut function = Function::new([]);
    for instruction in [
        I::GlobalGet(PHASE_GLOBAL),
        I::If(BlockType::Empty),
        I::I32Const(0),
        I::Return,
        I::End,
    ] {
        function.instruction(&instruction);
    }
    for module in &manifest.data_modules {
        let key = plan
            .globals
            .iter()
            .find(|(symbol, _)| symbol.index() == module.symbol as usize)
            .unwrap()
            .1;
        let offset = plan.demands[key];
        for instruction in [
            I::LocalGet(0),
            I::I32Const(module.symbol as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::I32Const(offset as i32),
            I::I32Load(memory(0, 2)),
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
            I::LocalGet(1),
            I::I32Load(memory(TYPE, 2)),
            I::I32Const(module.ty as i32),
            I::I32Ne,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
            I::I32Const(offset as i32),
            I::LocalGet(1),
            I::I32Store(memory(4, 2)),
            I::I32Const(offset as i32),
            I::I32Const(2),
            I::I32Store(memory(0, 2)),
            I::I32Const(1),
            I::Return,
            I::End,
        ] {
            function.instruction(&instruction);
        }
    }
    function.instruction(&I::I32Const(0)).instruction(&I::End);
    function
}

impl Session {
    pub fn register_data_sources(
        &mut self,
        sources: &telora_core::SourceDatabase,
        plan: &ValidatedDataPlan,
    ) {
        for file in sources.files() {
            if !self
                .manifest
                .sources
                .iter()
                .any(|source| source.id == file.id().get())
            {
                self.manifest.sources.push(crate::artifact::Source {
                    id: file.id().get(),
                    name: file.name.to_string(),
                });
            }
        }
        for node in plan.nodes() {
            let mut locations = vec![node.location];
            if let DataPlanNodeKind::Object(fields) = &node.kind {
                locations.extend(fields.values().map(|f| f.key_location));
            }
            for loc in locations {
                let file = sources.get(loc.source);
                let start = file.position(loc.start);
                let end = file.position(loc.end);
                self.manifest.locations.push(crate::artifact::Location {
                    source: loc.source.get(),
                    start: loc.start,
                    end: loc.end,
                    line: start.line,
                    column: start.column,
                    end_line: end.line,
                    end_column: end.column,
                });
            }
        }
    }
    pub fn inject_data(&mut self, symbol: u32, plan: &ValidatedDataPlan) -> Result<(), String> {
        if !self
            .manifest
            .data_modules
            .iter()
            .any(|module| module.symbol == symbol)
        {
            return Err("Wasm: data module is not in the executable".into());
        }
        let pointer = self.materialize_data(plan)?;
        let inject = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_inject_data")
            .map_err(|e| e.to_string())?;
        if inject
            .call(&mut self.store, (symbol as i32, pointer as i32))
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err(
                "Wasm: data module must be injected exactly once before initialization".into(),
            );
        }
        Ok(())
    }
    pub(crate) fn materialize_data(&mut self, plan: &ValidatedDataPlan) -> Result<u32, String> {
        let root = plan.root_node().ok_or("Wasm: missing data root")?;
        self.data_node(
            plan,
            root,
            &mut vec![None; plan.nodes().len()],
            &mut vec![false; plan.nodes().len()],
            0,
        )
    }
    fn data_node(
        &mut self,
        plan: &ValidatedDataPlan,
        id: DataNodeId,
        cache: &mut [Option<u32>],
        visiting: &mut [bool],
        depth: usize,
    ) -> Result<u32, String> {
        if depth > 512 {
            return Err("Wasm: data nesting limit".into());
        }
        if let Some(value) = cache[id.index()] {
            return Ok(value);
        }
        if visiting[id.index()] {
            return Err("Wasm: cyclic data plan".into());
        }
        visiting[id.index()] = true;
        let node = &plan.nodes()[id.index()];
        let value_ty = self
            .manifest
            .value_type
            .ok_or("Wasm: semantic Value contract missing")?;
        let desc = self.manifest.types[value_ty as usize].clone();
        let tag = match &node.kind {
            DataPlanNodeKind::Scalar(value) => match value {
                DataScalar::Int(_) => "Int",
                DataScalar::Float(_) => "Float",
                DataScalar::String(_) => "String",
                DataScalar::Bytes(_) => "Bytes",
                DataScalar::Null => "None",
                DataScalar::Bool(true) => "True",
                DataScalar::Bool(false) => "False",
                DataScalar::Temporal { kind, .. } => kind.variant(),
            },
            DataPlanNodeKind::Array(_) => "Array",
            DataPlanNodeKind::Object(_) => "Object",
        };
        let index = desc
            .variants
            .iter()
            .position(|variant| variant.name == tag)
            .ok_or("Wasm: data variant is not in semantic Value")?;
        let branch = &desc.variants[index];
        let payload = if let Some(ty) = branch.ty {
            Some(match &node.kind {
                DataPlanNodeKind::Scalar(scalar) => match scalar {
                    DataScalar::Int(value) => self.input(ty, &(*value).into(), 0)?,
                    DataScalar::Float(value) => self.input(
                        ty,
                        &serde_json::Number::from_f64(*value)
                            .ok_or("Wasm: non-finite data Float")?
                            .into(),
                        0,
                    )?,
                    DataScalar::String(value) | DataScalar::Temporal { value, .. } => {
                        self.input(ty, &value.clone().into(), 0)?
                    }
                    DataScalar::Bytes(bytes) => self.input_bytes(ty, bytes)?,
                    _ => return Err("Wasm: invalid data scalar payload".into()),
                },
                DataPlanNodeKind::Array(items) => {
                    let mut values = Vec::with_capacity(items.len());
                    for &item in items {
                        values.push(self.data_node(plan, item, cache, visiting, depth + 1)?);
                    }
                    self.input_array_values(ty, &values)?
                }
                DataPlanNodeKind::Object(fields) => {
                    let string = self
                        .manifest
                        .types
                        .iter()
                        .position(|t| t.kind == Kind::String)
                        .ok_or("Wasm: missing String")? as u32;
                    let mut values = vec![];
                    for (name, field) in fields {
                        let key = self.input(string, &name.clone().into(), 0)?;
                        self.input_location(key, field.key_location)?;
                        values.push((
                            key,
                            self.data_node(plan, field.value, cache, visiting, depth + 1)?,
                        ));
                    }
                    self.input_dict_values(ty, &values)?
                }
            })
        } else {
            None
        };
        if let Some(payload) = payload {
            self.input_location(payload, node.location)?;
        }
        let result = self.input_variant(value_ty, index, payload)?;
        self.input_location(result, node.location)?;
        visiting[id.index()] = false;
        cache[id.index()] = Some(result);
        Ok(result)
    }
    fn input_location(
        &mut self,
        pointer: u32,
        location: telora_core::Location,
    ) -> Result<(), String> {
        for (offset, value) in [location.source.get(), location.start, location.end]
            .into_iter()
            .enumerate()
        {
            self.write(pointer as usize + offset * 4, &value.to_le_bytes())?;
        }
        Ok(())
    }
}
