use super::*;

fn parse(source: &str) -> TomlParse {
    let mut sources = SourceDatabase::default();
    let id = sources.add("test.toml", source);
    parse_toml_registered(&sources, id)
}

include!("part-01.rs");
