//! Source-backed data transport. Materialize once, then inject before initialization.
use crate::data_packet::DataPacket;
use crate::data_view::{Graph, Value};
use crate::{
    abi::*,
    artifact::{Kind, Manifest},
    plan::Plan,
    session::Session,
};
use telora_core::data_plan::ParsedData;
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

impl Manifest {
    pub fn register_data_sources(
        &mut self,
        sources: &telora_core::SourceDatabase,
        plan: &ParsedData,
    ) -> Result<(), String> {
        let mut ids = std::collections::BTreeSet::new();
        match plan {
            ParsedData::Json { plan, .. } => {
                ids.extend(plan.nodes.iter().map(|node| node.location.source));
            }
            ParsedData::Yaml { plan, .. } => {
                ids.extend(plan.nodes.iter().map(|node| node.location.source));
            }
            ParsedData::Toml { plan, .. } => {
                ids.extend(plan.nodes.iter().map(|node| node.location.source));
            }
        }
        for id in ids {
            let file = sources.files().find(|file| file.id() == id)
                .ok_or("Wasm: data plan references an unregistered source")?;
            if self
                .sources
                .iter()
                .any(|source| source.id == file.id().get() && source.name != file.name.as_ref())
            {
                return Err(
                    "Wasm: data source identity conflicts with the compiled source database".into(),
                );
            }
            if !self
                .sources
                .iter()
                .any(|source| source.id == file.id().get())
            {
                self.sources.push(crate::artifact::Source::from_file(file));
            }
        }
        Ok(())
    }
}

impl Session {
    pub fn register_data_sources(
        &mut self,
        sources: &telora_core::SourceDatabase,
        plan: &ParsedData,
    ) -> Result<(), String> {
        self.manifest.register_data_sources(sources, plan)?;
        self.register_sources()
    }
    pub fn inject_data(&mut self, symbol: u32, plan: &ParsedData, sources: &telora_core::SourceDatabase) -> Result<(), String> {
        self.inject_graph(symbol, Graph::parsed(plan, sources)?)
    }
    pub fn inject_data_packet(&mut self, symbol: u32, plan: &DataPacket) -> Result<(), String> {
        plan.validate(&self.manifest)?;
        self.inject_graph(symbol, Graph::Packet(plan))
    }
    fn inject_graph(&mut self, symbol: u32, plan: Graph<'_>) -> Result<(), String> {
        if !self
            .manifest
            .data_modules
            .iter()
            .any(|module| module.symbol == symbol)
        {
            return Err("Wasm: data module is not in the executable".into());
        }
        let pointer = self.materialize_graph(plan)?;
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
    pub(crate) fn materialize_data(&mut self, plan: &ParsedData, sources: &telora_core::SourceDatabase) -> Result<u32, String> {
        self.materialize_graph(Graph::parsed(plan, sources)?)
    }
    pub fn materialize_value(&mut self, plan: &ParsedData, sources: &telora_core::SourceDatabase) -> Result<crate::transport::Value, String> {
        let ty = self.manifest.value_type.ok_or("Wasm: missing semantic Value type")?;
        Ok(crate::transport::Value {pointer: self.materialize_data(plan, sources)?, ty})
    }
    fn materialize_graph(&mut self, plan: Graph<'_>) -> Result<u32, String> {
        self.data_node(
            plan,
            plan.root()?,
            &mut vec![None; plan.len()],
            &mut vec![false; plan.len()],
            0,
        )
    }
    fn data_node(
        &mut self,
        plan: Graph<'_>,
        id: usize,
        cache: &mut [Option<u32>],
        visiting: &mut [bool],
        depth: usize,
    ) -> Result<u32, String> {
        if depth > 512 {
            return Err("Wasm: data nesting limit".into());
        }
        if let Some(value) = cache[id as usize] {
            return Ok(value);
        }
        if visiting[id as usize] {
            return Err("Wasm: cyclic data plan".into());
        }
        visiting[id as usize] = true;
        let node = plan.node(id)?;
        let value_ty = self
            .manifest
            .value_type
            .ok_or("Wasm: semantic Value contract missing")?;
        let desc = &self.manifest.types[value_ty as usize];
        let tag = match &node.value {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::String(_) => "String",
            Value::Bytes(_) => "Bytes",
            Value::Null => "None",
            Value::Bool(true) => "True",
            Value::Bool(false) => "False",
            Value::Temporal { variant, .. } => variant,
            Value::Array(_) => "Array",
            Value::Object(_) => "Object",
        };
        let index = desc
            .variants
            .iter()
            .position(|variant| variant.name == tag)
            .ok_or("Wasm: data variant is not in semantic Value")?;
        let branch = &desc.variants[index];
        let payload = if let Some(ty) = branch.ty {
            Some(match node.value {
                Value::Int(value) => self.input(ty, &value.into(), 0)?,
                Value::Float(value) => self.input(
                    ty,
                    &serde_json::Number::from_f64(value)
                        .ok_or("Wasm: non-finite data Float")?
                        .into(),
                    0,
                )?,
                Value::String(value) | Value::Temporal { value, .. } => {
                    self.input_text(ty, value)?
                }
                Value::Bytes(bytes) => self.input_bytes(ty, bytes)?,
                Value::Array(items) => {
                    let mut values = Vec::with_capacity(items.len());
                    for item in items {
                        values.push(self.data_node(plan, item, cache, visiting, depth + 1)?);
                    }
                    self.input_array_values(ty, &values)?
                }
                Value::Object(fields) => {
                    let string = self
                        .manifest
                        .types
                        .iter()
                        .position(|t| t.kind == Kind::String)
                        .ok_or("Wasm: missing String")? as u32;
                    let mut values = vec![];
                    for field in fields {
                        let key = self.input_text(string, field.name)?;
                        self.input_location(key, field.origin)?;
                        values.push((
                            key,
                            self.data_node(plan, field.value, cache, visiting, depth + 1)?,
                        ));
                    }
                    self.input_dict_values(ty, &values)?
                }
                _ => return Err("Wasm: invalid data scalar payload".into()),
            })
        } else {
            None
        };
        if let Some(payload) = payload {
            self.input_location(payload, node.origin)?;
        }
        let result = self.input_variant(value_ty, index, payload)?;
        self.input_location(result, node.origin)?;
        visiting[id as usize] = false;
        cache[id as usize] = Some(result);
        Ok(result)
    }
    fn input_location(&mut self, pointer: u32, location: [u32; 3]) -> Result<(), String> {
        for (offset, value) in location.into_iter().enumerate() {
            self.write(pointer as usize + offset * 4, &value.to_le_bytes())?;
        }
        Ok(())
    }
}
