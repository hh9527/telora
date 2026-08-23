use super::evaluate_expression_module as load_module;
use super::evaluate_expression_module_with_quota_and_debug_sink as load_module_with_quota_and_debug_sink;
use super::*;
use crate::parse_json;
use std::sync::Mutex;

fn module_blueprint(source: &str) -> Result<ModuleBlueprint, String> {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add("@test/skeleton.telora", source);
    let parsed = parse_registered(&sources, source_id);
    let program = parsed.program.expect("skeleton fixture must parse");
    ModuleBlueprint::from_program(&program)
}

include!("part-01.rs");
include!("part-02.rs");
include!("part-03.rs");
include!("part-04.rs");
include!("part-05.rs");
include!("part-06.rs");
include!("part-07.rs");
include!("part-08.rs");
