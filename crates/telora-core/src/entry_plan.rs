//! Static service entry plan, shared by execution backends before code generation.

/// MainService is a type export; ordinary trait evidence closes both methods.
pub fn transform_adapter(module: &str) -> Result<String, String> {
    if module.is_empty() {
        return Err("service module name is empty".into());
    }
    Ok(format!(
        r#"
        mod application;
        use self::application::{{ MainService }};
        use std::_entry::transform as entry;
        pub def main: entry::Plan = entry::prepare(MainService.type);
    "#
    ))
}
