use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use telora_core::SystemEesModel;
use telora_ees::{ComponentSpec, ImosSpec, Manifest, ResourceLocator, SqliteQuerySpec};

#[derive(Clone, Debug)]
pub(crate) struct NamedEesVar {
    name: String,
    value: String,
}

pub(crate) struct CollectedEes {
    pub(crate) manifest: Option<Manifest>,
    pub(crate) actors: BTreeMap<String, String>,
}

pub(crate) fn parse_named_ees_var(value: &str) -> Result<NamedEesVar, String> {
    let (name, value) = value
        .split_once('=')
        .ok_or_else(|| "EES variable must use NAME=VALUE".to_owned())?;
    if !valid_name(name) {
        return Err(format!("invalid EES variable name {name:?}"));
    }
    if value.is_empty() {
        return Err(format!("EES variable {name:?} must not be empty"));
    }
    if value.contains(['/', '\\', '\0']) || matches!(value, "." | "..") {
        return Err(format!(
            "EES variable {name:?} must contain one safe path segment"
        ));
    }
    Ok(NamedEesVar {
        name: name.into(),
        value: value.into(),
    })
}

pub(crate) fn collect_ees_models(
    patterns: &BTreeMap<String, String>,
    models: &[SystemEesModel],
    bindings: Vec<NamedEesVar>,
) -> Result<CollectedEes, String> {
    let patterns = patterns
        .iter()
        .map(|(name, pattern)| {
            if !valid_name(name) {
                return Err(format!("invalid EES variable name {name:?}"));
            }
            Regex::new(&format!("^(?:{pattern})$"))
                .map(|regex| (name.clone(), regex))
                .map_err(|error| format!("invalid EES variable pattern for {name:?}: {error}"))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let values = validate_bindings(&patterns, bindings)?;
    let mut actors = BTreeMap::new();
    let mut components = Vec::with_capacity(models.len());
    let mut used = BTreeSet::new();
    for model in models {
        if !valid_name(&model.name) {
            return Err(format!("invalid EES actor name {:?}", model.name));
        }
        let config = model
            .config
            .as_object()
            .ok_or_else(|| format!("EES model {:?} config must be an object", model.name))?;
        let string = |field: &str| {
            config
                .get(field)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("EES model {:?} config.{field} must be String", model.name))
        };
        let spec = match model.kind.as_str() {
            "imos" => {
                if config.len() != 2
                    || !config.contains_key("home")
                    || !config.contains_key("store")
                {
                    return Err(format!(
                        "EES model {:?} imos config must contain exactly home and store",
                        model.name
                    ));
                }
                let store =
                    ResourceLocator::user(expand_template(string("store")?, &values, &mut used)?)
                        .map_err(|error| {
                        format!("invalid EES model {:?} store: {error:#}", model.name)
                    })?;
                let home =
                    ResourceLocator::user(expand_template(string("home")?, &values, &mut used)?)
                        .map_err(|error| {
                            format!("invalid EES model {:?} home: {error:#}", model.name)
                        })?;
                ComponentSpec::Imos(ImosSpec {
                    name: model.name.clone(),
                    store,
                    home,
                })
            }
            "sqlite-query" => {
                if config.len() != 1 || !config.contains_key("path") {
                    return Err(format!(
                        "EES model {:?} sqlite-query config must contain exactly path",
                        model.name
                    ));
                }
                let database =
                    ResourceLocator::user(expand_template(string("path")?, &values, &mut used)?)
                        .map_err(|error| {
                            format!("invalid EES model {:?} path: {error:#}", model.name)
                        })?;
                ComponentSpec::SqliteQuery(SqliteQuerySpec {
                    name: model.name.clone(),
                    database,
                })
            }
            kind => return Err(format!("unsupported EES model kind {kind:?}")),
        };
        if actors
            .insert(model.name.clone(), spec.kind().as_str().to_owned())
            .is_some()
        {
            return Err(format!(
                "EES actor {:?} is declared more than once",
                model.name
            ));
        }
        components.push(spec);
    }
    let unused = patterns
        .keys()
        .filter(|name| !used.contains(*name))
        .cloned()
        .collect::<Vec<_>>();
    if !unused.is_empty() {
        return Err(format!("EES variables are declared but unused: {unused:?}"));
    }
    Ok(CollectedEes {
        manifest: (!components.is_empty()).then(|| Manifest::new(components)),
        actors,
    })
}

fn validate_bindings(
    patterns: &BTreeMap<String, Regex>,
    bindings: Vec<NamedEesVar>,
) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for binding in bindings {
        let pattern = patterns.get(&binding.name).ok_or_else(|| {
            format!(
                "EES variable {:?} is not declared by the selected ees.Config value",
                binding.name
            )
        })?;
        if !pattern.is_match(&binding.value) {
            return Err(format!(
                "EES variable {:?} does not match its declared pattern {:?}",
                binding.name,
                pattern.as_str()
            ));
        }
        if values.insert(binding.name.clone(), binding.value).is_some() {
            return Err(format!(
                "EES variable {:?} was provided more than once",
                binding.name
            ));
        }
    }
    let missing = patterns
        .keys()
        .filter(|name| !values.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("EES variables were not provided: {missing:?}"));
    }
    Ok(values)
}

fn expand_template(
    template: &str,
    variables: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
) -> Result<String, String> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        output.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        let close = tail
            .find('}')
            .ok_or_else(|| format!("unclosed EES variable in locator {template:?}"))?;
        let name = &tail[..close];
        let value = variables
            .get(name)
            .ok_or_else(|| format!("EES locator references undeclared variable {name:?}"))?;
        output.push_str(value);
        used.insert(name.into());
        rest = &tail[close + 1..];
    }
    if rest.contains('}') {
        return Err(format!(
            "unmatched closing brace in EES locator {template:?}"
        ));
    }
    output.push_str(rest);
    Ok(output)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_safe_cli_variables() {
        assert_eq!(
            parse_named_ees_var("tenant=hello-1").unwrap().name,
            "tenant"
        );
        for value in ["bad", "Tenant=x", "tenant=", "tenant=../x", "tenant=a/b"] {
            assert!(parse_named_ees_var(value).is_err(), "{value}");
        }
    }

    #[test]
    fn expands_declared_variables() {
        let values = BTreeMap::from([("tenant".into(), "hello".into())]);
        let mut used = BTreeSet::new();
        assert_eq!(
            expand_template("user-data:catalog/{tenant}/db.sqlite", &values, &mut used).unwrap(),
            "user-data:catalog/hello/db.sqlite"
        );
        assert!(used.contains("tenant"));
    }
}
