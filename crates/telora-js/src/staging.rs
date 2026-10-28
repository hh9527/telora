//! Build-time evaluation support for direct JavaScript artifacts.
//! Evaluation and graph transfer run in a separate, memory-limited Node process.
pub const PREPARE: &str = include_str!("../host/prepare.mjs");

/// Remove generated function/global definitions unreachable from the restored
/// graph or conservative property/checker roots. Type indices stay unchanged.
pub fn residual(source: &str, image: &str, service: bool, full: bool) -> Result<String, String> {
    use std::collections::{BTreeMap, BTreeSet};
    let archive: serde_json::Value = serde_json::from_str(image).map_err(|e| e.to_string())?;
    if archive["schema"] != "telora.js.archive/v1" {
        return Err("unsupported JS build archive".into());
    }
    let image = serde_json::to_string(&archive["image"]).map_err(|e| e.to_string())?;
    let mut information = String::new();
    for (key, value) in archive["information"]
        .as_object()
        .ok_or("missing build information")?
    {
        let key = serde_json::to_string(key).map_err(|e| e.to_string())?;
        let json = serde_json::to_string(&serde_json::to_string(value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        information.push_str(&format!("Object.defineProperty(buildInformation,{key},{{enumerable:true,get:()=>JSON.parse({json})}});\n"));
    }
    let mut definitions = BTreeMap::new();
    let mut roots = String::new();
    let mut original = None;
    for line in source.lines() {
        if line.starts_with("const entry = ") {
            original = Some(line);
        } else if line.starts_with("const g") || line.starts_with("stageFactories[") {
            if let Some(name) = symbols(line).into_iter().next() {
                definitions.insert(name, line);
            }
        } else {
            roots.push_str(line);
            roots.push('\n');
        }
    }
    original.ok_or("missing JS entry")?;
    let mut used = BTreeSet::new();
    let mut pending = symbols(&roots);
    pending.extend(symbols(&image));
    while let Some(name) = pending.pop() {
        if used.insert(name.clone()) {
            if let Some(code) = definitions.get(&name) {
                pending.extend(symbols(code));
            }
        }
    }
    let mut output = String::from("const stageRuntimeOnly=true;\n");
    for line in source.lines() {
        if Some(line) == original {
            let image = serde_json::to_string(&image).map_err(|e| e.to_string())?;
            let runtime = if service {
                "[[], () => stagedRoot]"
            } else {
                "stagedRoot"
            };
            output.push_str(&format!("const stagedRoot=stageUnpack(JSON.parse({image}));\n{information}const entry = {runtime};\n"));
        } else if line.starts_with("const g") || line.starts_with("stageFactories[") {
            if full
                || symbols(line)
                    .first()
                    .is_some_and(|name| used.contains(name))
            {
                output.push_str(line);
                output.push('\n');
            }
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    Ok(output)
}

// Matching generated identifiers in literals as well is conservative: it can
// retain an extra definition, but cannot discard a real code dependency.
fn symbols(source: &str) -> Vec<String> {
    source
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|token| {
            let Some(rest) = token.strip_prefix('g').or_else(|| token.strip_prefix('c')) else {
                return false;
            };
            let Some((index, instance)) = rest.split_once('_') else {
                return false;
            };
            !index.is_empty()
                && index.bytes().all(|b| b.is_ascii_digit())
                && (instance == "v"
                    || (!instance.is_empty() && instance.bytes().all(|b| b.is_ascii_digit())))
        })
        .map(str::to_owned)
        .collect()
}
