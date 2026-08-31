use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use imos::Store;
use imos::progress::ProgressSender;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlite_query::{Database, Query};

pub mod stdio;

#[derive(Clone, Debug)]
pub struct Manifest {
    components: Vec<ComponentSpec>,
}

impl Manifest {
    pub fn new(components: Vec<ComponentSpec>) -> Self {
        Self { components }
    }

    pub fn components(&self) -> &[ComponentSpec] {
        &self.components
    }
}

#[derive(Clone, Debug)]
pub enum ComponentSpec {
    Imos(ImosSpec),
    SqliteQuery(SqliteQuerySpec),
}

impl ComponentSpec {
    pub fn name(&self) -> &str {
        match self {
            Self::Imos(spec) => &spec.name,
            Self::SqliteQuery(spec) => &spec.name,
        }
    }

    pub fn kind(&self) -> ComponentKind {
        match self {
            Self::Imos(_) => ComponentKind::Imos,
            Self::SqliteQuery(_) => ComponentKind::SqliteQuery,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentKind {
    Imos,
    SqliteQuery,
}

impl ComponentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Imos => "imos",
            Self::SqliteQuery => "sqlite-query",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImosSpec {
    pub name: String,
    pub store: PathBuf,
    pub home: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SqliteQuerySpec {
    pub name: String,
    pub database: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Call {
    pub id: String,
    pub actor: String,
    pub operation: String,
    pub input: Value,
}

impl Call {
    pub fn install_shared(id: impl Into<String>, actor: impl Into<String>, plan: Value) -> Self {
        Self {
            id: id.into(),
            actor: actor.into(),
            operation: "InstallShared".into(),
            input: json!({"plan": plan}),
        }
    }

    pub fn sqlite_query(
        id: impl Into<String>,
        actor: impl Into<String>,
        sql: impl Into<String>,
        bindings: Vec<Value>,
    ) -> Self {
        Self {
            id: id.into(),
            actor: actor.into(),
            operation: "Query".into(),
            input: json!({"sql": sql.into(), "bindings": bindings}),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TerminalEvent {
    Result { id: String, value: Value },
    Error { id: Option<String>, message: String },
}

impl TerminalEvent {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Result { id, .. } => Some(id),
            Self::Error { id, .. } => id.as_deref(),
        }
    }

    pub fn into_value(self) -> std::result::Result<Value, String> {
        match self {
            Self::Result { value, .. } => Ok(value),
            Self::Error { message, .. } => Err(message),
        }
    }
}

enum Component {
    Imos { store: Store, home: PathBuf },
    SqliteQuery(Arc<Mutex<Database>>),
}

impl Component {
    fn kind(&self) -> ComponentKind {
        match self {
            Self::Imos { .. } => ComponentKind::Imos,
            Self::SqliteQuery(_) => ComponentKind::SqliteQuery,
        }
    }
}

#[derive(Clone)]
pub struct Service {
    components: Arc<BTreeMap<String, Component>>,
}

impl Service {
    pub async fn open(manifest: Manifest) -> Result<Self> {
        if manifest.components.is_empty() {
            bail!("EES manifest must contain at least one component");
        }
        let mut components = BTreeMap::new();
        for spec in manifest.components {
            let name = spec.name().to_owned();
            if name.is_empty() {
                bail!("EES component name must not be empty");
            }
            if components.contains_key(&name) {
                bail!("EES component name {name:?} is duplicated");
            }
            let component = match spec {
                ComponentSpec::Imos(spec) => Component::Imos {
                    store: Store::open(spec.store)
                        .await
                        .with_context(|| format!("cannot open IMOS actor {name:?}"))?,
                    home: spec.home,
                },
                ComponentSpec::SqliteQuery(spec) => {
                    let actor_name = name.clone();
                    let database =
                        tokio::task::spawn_blocking(move || Database::open(spec.database))
                            .await
                            .with_context(|| {
                                format!("SQLite actor {actor_name:?} construction task failed")
                            })??;
                    Component::SqliteQuery(Arc::new(Mutex::new(database)))
                }
            };
            components.insert(name, component);
        }
        Ok(Self {
            components: Arc::new(components),
        })
    }

    pub fn actors(&self) -> BTreeMap<String, ComponentKind> {
        self.components
            .iter()
            .map(|(name, component)| (name.clone(), component.kind()))
            .collect()
    }

    pub async fn dispatch(&self, call: Call, progress: Option<ProgressSender>) -> TerminalEvent {
        let id = call.id.clone();
        let result = self.dispatch_inner(call, progress).await;
        match result {
            Ok(value) => TerminalEvent::Result { id, value },
            Err(error) => TerminalEvent::Error {
                id: Some(id),
                message: format!("{error:#}"),
            },
        }
    }

    async fn dispatch_inner(&self, call: Call, progress: Option<ProgressSender>) -> Result<Value> {
        let component = self
            .components
            .get(&call.actor)
            .with_context(|| format!("EES actor {:?} is not configured", call.actor))?;
        match component {
            Component::Imos { store, home } => {
                if call.operation != "InstallShared" {
                    bail!(
                        "EES actor {:?} has kind imos and does not support operation {:?}",
                        call.actor,
                        call.operation
                    );
                }
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    plan: Value,
                }
                let input: Input = serde_json::from_value(call.input)
                    .context("InstallShared input must contain only plan")?;
                let root = match progress {
                    Some(progress) => {
                        store
                            .install_with_progress(home, input.plan, progress)
                            .await
                    }
                    None => store.install(home, input.plan).await,
                }?;
                Ok(json!({"root": root.to_string_lossy()}))
            }
            Component::SqliteQuery(database) => {
                if call.operation != "Query" {
                    bail!(
                        "EES actor {:?} has kind sqlite-query and does not support operation {:?}",
                        call.actor,
                        call.operation
                    );
                }
                let query: Query = serde_json::from_value(call.input)
                    .context("Query input must contain only sql and bindings")?;
                let database = Arc::clone(database);
                let output = tokio::task::spawn_blocking(move || {
                    database
                        .lock()
                        .map_err(|_| anyhow::anyhow!("SQLite actor lock is poisoned"))?
                        .query(query)
                })
                .await
                .context("SQLite query task failed")??;
                serde_json::to_value(output).context("cannot encode SQLite query output")
            }
        }
    }
}

pub fn default_store_path() -> Result<PathBuf> {
    Ok(ProjectDirs::from("dev", "imos", "imos")
        .context("could not determine the EES store directory; pass --store explicitly")?
        .cache_dir()
        .to_path_buf())
}

pub fn configured_store_path() -> Result<PathBuf> {
    match env::var_os("TELORA_EES_STORE") {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        Some(_) => bail!("TELORA_EES_STORE must not be empty"),
        None => default_store_path(),
    }
}

pub fn imos_manifest(
    name: impl Into<String>,
    store: impl AsRef<Path>,
    home: impl AsRef<Path>,
) -> Manifest {
    Manifest::new(vec![ComponentSpec::Imos(ImosSpec {
        name: name.into(),
        store: store.as_ref().to_path_buf(),
        home: home.as_ref().to_path_buf(),
    })])
}

pub fn dispatch_blocking(manifest: Manifest, calls: Vec<Call>) -> Result<Vec<TerminalEvent>> {
    thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot start the embedded EES runtime")?
            .block_on(async {
                let service = Service::open(manifest).await?;
                let mut events = Vec::with_capacity(calls.len());
                for call in calls {
                    events.push(service.dispatch(call, None).await);
                }
                Result::<_>::Ok(events)
            })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("embedded EES runtime panicked"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_serializes_an_explicit_null_id() {
        let event = TerminalEvent::Error {
            id: None,
            message: "invalid request".into(),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({"type": "error", "id": null, "message": "invalid request"})
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manifest_rejects_empty_and_duplicate_names() {
        let root = tempfile::tempdir().unwrap();
        let specs = |first: &str, second: &str| {
            Manifest::new(vec![
                ComponentSpec::Imos(ImosSpec {
                    name: first.into(),
                    store: root.path().join("store-1"),
                    home: root.path().join("home-1"),
                }),
                ComponentSpec::Imos(ImosSpec {
                    name: second.into(),
                    store: root.path().join("store-2"),
                    home: root.path().join("home-2"),
                }),
            ])
        };
        assert!(Service::open(specs("", "b")).await.is_err());
        assert!(Service::open(specs("same", "same")).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatches_by_actor_kind_and_rejects_mismatched_operations() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("data.sqlite");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE item(value INTEGER); INSERT INTO item VALUES (3);")
            .unwrap();
        let service = Service::open(Manifest::new(vec![ComponentSpec::SqliteQuery(
            SqliteQuerySpec {
                name: "db".into(),
                database: path,
            },
        )]))
        .await
        .unwrap();
        let result = service
            .dispatch(
                Call::sqlite_query("q", "db", "SELECT value FROM item", vec![]),
                None,
            )
            .await
            .into_value()
            .unwrap();
        assert_eq!(result["columns"], json!(["value"]));
        assert_eq!(result["rows"], json!([[3]]));

        let error = service
            .dispatch(Call::install_shared("bad", "db", json!({})), None)
            .await
            .into_value()
            .unwrap_err();
        assert!(error.contains("does not support operation"));
    }
}
