use super::*;
fn parse(source: &str) -> YamlParse {
    let mut sources = SourceDatabase::default();
    let id = sources.add("test.yaml", source);
    parse_yaml_registered(&sources, id)
}
include!("part-01.rs");
