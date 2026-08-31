use std::collections::BTreeMap;
use std::path::PathBuf;

use telora_ees::{ComponentKind, ComponentSpec, ImosSpec, Manifest, SqliteQuerySpec};

#[derive(Clone)]
pub(crate) struct NamedEes {
    name: String,
    spec: ComponentSpec,
}

pub(crate) struct CollectedEes {
    pub(crate) manifest: Option<Manifest>,
    pub(crate) actors: BTreeMap<String, String>,
}

pub(crate) fn parse_named_ees(value: &str) -> Result<NamedEes, String> {
    let (identity, locator) = value
        .split_once('=')
        .ok_or_else(|| "EES binding must use KIND:NAME=LOCATOR".to_owned())?;
    let (kind, name) = identity
        .split_once(':')
        .ok_or_else(|| "EES binding must use KIND:NAME=LOCATOR".to_owned())?;
    if name.is_empty() {
        return Err("EES actor name must not be empty".into());
    }
    if locator.is_empty() {
        return Err("EES actor locator must not be empty".into());
    }
    let spec = match kind {
        "imos" => {
            let root = PathBuf::from(locator);
            ComponentSpec::Imos(ImosSpec {
                name: name.into(),
                store: root.join("store"),
                home: root.join("home"),
            })
        }
        "sqlite-query" => {
            let path = locator
                .strip_prefix("sqlite://")
                .ok_or_else(|| "sqlite-query actor locator must use sqlite://PATH".to_owned())?;
            if path.is_empty() {
                return Err("sqlite-query actor locator must contain a database path".into());
            }
            ComponentSpec::SqliteQuery(SqliteQuerySpec {
                name: name.into(),
                database: PathBuf::from(path),
            })
        }
        _ => return Err(format!("unsupported EES component kind {kind:?}")),
    };
    Ok(NamedEes {
        name: name.into(),
        spec,
    })
}

pub(crate) fn collect_ees(bindings: Vec<NamedEes>) -> Result<CollectedEes, String> {
    let mut actors = BTreeMap::new();
    let mut components = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let kind = match binding.spec.kind() {
            ComponentKind::Imos => {
                if let ComponentSpec::Imos(spec) = &binding.spec {
                    std::fs::create_dir_all(&spec.home).map_err(|error| {
                        format!(
                            "cannot create IMOS actor {:?} home {}: {error}",
                            binding.name,
                            spec.home.display()
                        )
                    })?;
                }
                ComponentKind::Imos
            }
            ComponentKind::SqliteQuery => ComponentKind::SqliteQuery,
        };
        if actors
            .insert(binding.name.clone(), kind.as_str().to_owned())
            .is_some()
        {
            return Err(format!(
                "EES actor {:?} was provided more than once",
                binding.name
            ));
        }
        components.push(binding.spec);
    }
    Ok(CollectedEes {
        manifest: (!components.is_empty()).then(|| Manifest::new(components)),
        actors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_imos_roots_and_sqlite_uris() {
        let imos = parse_named_ees("imos:a=/tmp/a").unwrap();
        let ComponentSpec::Imos(spec) = imos.spec else {
            panic!("expected IMOS spec")
        };
        assert_eq!(spec.store, PathBuf::from("/tmp/a/store"));
        assert_eq!(spec.home, PathBuf::from("/tmp/a/home"));

        let sqlite = parse_named_ees("sqlite-query:db=sqlite:///tmp/data.sqlite").unwrap();
        let ComponentSpec::SqliteQuery(spec) = sqlite.spec else {
            panic!("expected SQLite spec")
        };
        assert_eq!(spec.database, PathBuf::from("/tmp/data.sqlite"));
    }

    #[test]
    fn rejects_invalid_and_duplicate_bindings() {
        for value in ["bad", "unknown:a=/tmp", "imos:=/tmp", "sqlite-query:a=x"] {
            assert!(parse_named_ees(value).is_err(), "{value}");
        }
        let root = tempfile::tempdir().unwrap();
        let first = parse_named_ees(&format!("imos:a={}", root.path().display())).unwrap();
        let second = parse_named_ees(&format!("imos:a={}", root.path().display())).unwrap();
        assert!(collect_ees(vec![first, second]).is_err());
    }
}
