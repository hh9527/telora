//! Static service entry plan, shared by execution backends before code generation.

/// MainService is a type export; ordinary trait evidence closes both methods.
pub fn transform_adapter(module: &str) -> Result<String, String> {
    let module = serde_json::to_string(module).map_err(|e| e.to_string())?;
    Ok(format!(
        r#"
        import {module} {{ MainService }};
        import "std/_entry/transform" as entry;
        export def main: entry.Plan = entry.prepare(MainService.type);
    "#
    ))
}
