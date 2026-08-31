use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use telora_core::{LoadedOptionAction, ValueRef};
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

pub(crate) fn collect_ees(
    options: &[LoadedOptionAction],
    bindings: Vec<NamedEesVar>,
) -> Result<CollectedEes, String> {
    let patterns = parse_patterns(options)?;
    let values = validate_bindings(&patterns, bindings)?;
    let mut actors = BTreeMap::new();
    let mut components = Vec::new();
    let mut used = BTreeSet::new();

    for option in options {
        let (name, spec) = match option.key.as_str() {
            "ees.imos" => parse_imos(option.value.value(), &values, &mut used)?,
            "ees.sqlite" => parse_sqlite(option.value.value(), &values, &mut used)?,
            _ => continue,
        };
        if !valid_name(&name) {
            return Err(format!("invalid EES actor name {name:?}"));
        }
        let kind = spec.kind();
        if actors.insert(name.clone(), kind.as_str().into()).is_some() {
            return Err(format!("EES actor {name:?} is declared more than once"));
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

fn parse_imos(
    value: ValueRef<'_>,
    variables: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
) -> Result<(String, ComponentSpec), String> {
    let context = "option \"ees.imos\"";
    let value = record(value, context)?;
    exact_fields(value, &["name", "home", "store"], context)?;
    let name = string_field(value, "name", context)?;
    let home = locator_field(value, "home", context, variables, used)?;
    let store = locator_field(value, "store", context, variables, used)?;
    Ok((
        name.clone(),
        ComponentSpec::Imos(ImosSpec { name, store, home }),
    ))
}

fn parse_sqlite(
    value: ValueRef<'_>,
    variables: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
) -> Result<(String, ComponentSpec), String> {
    let context = "option \"ees.sqlite\"";
    let value = record(value, context)?;
    exact_fields(value, &["name", "path"], context)?;
    let name = string_field(value, "name", context)?;
    let database = locator_field(value, "path", context, variables, used)?;
    Ok((
        name.clone(),
        ComponentSpec::SqliteQuery(SqliteQuerySpec { name, database }),
    ))
}

fn parse_patterns(options: &[LoadedOptionAction]) -> Result<BTreeMap<String, Regex>, String> {
    let declarations = options
        .iter()
        .filter(|option| option.key == "ees.vars")
        .collect::<Vec<_>>();
    let Some(declaration) = declarations.first() else {
        return Ok(BTreeMap::new());
    };
    if declarations.len() != 1 {
        return Err("option \"ees.vars\" may be declared only once".into());
    }
    let value = declaration.value.value();
    let fields = value
        .dict_fields()
        .ok_or_else(|| "option \"ees.vars\" must be a Dict(String)".to_owned())?;
    let mut patterns = BTreeMap::new();
    for name in fields {
        if !valid_name(name) {
            return Err(format!("invalid EES variable name {name:?}"));
        }
        let pattern = value
            .dict_get(name)
            .and_then(ValueRef::as_str)
            .ok_or_else(|| format!("EES variable pattern for {name:?} must be String"))?;
        let regex = Regex::new(&format!("^(?:{})$", pattern.as_str()))
            .map_err(|error| format!("invalid EES variable pattern for {name:?}: {error}"))?;
        patterns.insert(name.into(), regex);
    }
    Ok(patterns)
}

fn validate_bindings(
    patterns: &BTreeMap<String, Regex>,
    bindings: Vec<NamedEesVar>,
) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for binding in bindings {
        let pattern = patterns.get(&binding.name).ok_or_else(|| {
            format!(
                "EES variable {:?} is not declared by ees.vars",
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

fn locator_field(
    value: ValueRef<'_>,
    field: &str,
    context: &str,
    variables: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
) -> Result<ResourceLocator, String> {
    let template = string_field(value, field, context)?;
    let expanded = expand_template(&template, variables, used)?;
    ResourceLocator::user(expanded)
        .map_err(|error| format!("invalid {context}.{field} locator: {error:#}"))
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

fn record<'a>(value: ValueRef<'a>, context: &str) -> Result<ValueRef<'a>, String> {
    value
        .dict_fields()
        .is_some()
        .then_some(value)
        .ok_or_else(|| format!("{context} must be a record"))
}

fn exact_fields(value: ValueRef<'_>, expected: &[&str], context: &str) -> Result<(), String> {
    let actual = value
        .dict_fields()
        .expect("record was checked")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "{context} must contain exactly fields {expected:?}; found {actual:?}"
        ));
    }
    Ok(())
}

fn string_field(value: ValueRef<'_>, field: &str, context: &str) -> Result<String, String> {
    value
        .dict_get(field)
        .and_then(ValueRef::as_str)
        .map(String::from)
        .ok_or_else(|| format!("{context}.{field} must be String"))
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
