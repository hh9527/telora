use std::path::{Path, PathBuf};
use std::{env, thread};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use imos::Store;
use imos::progress::ProgressSender;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod stdio;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    InstallShared(InstallSharedRequest),
}

impl Request {
    pub fn id(&self) -> &str {
        match self {
            Self::InstallShared(request) => &request.id,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallSharedRequest {
    pub id: String,
    pub home: PathBuf,
    pub plan: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TerminalEvent {
    Result { id: String, root: String },
    Error { id: Option<String>, message: String },
}

impl TerminalEvent {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Result { id, .. } => Some(id),
            Self::Error { id, .. } => id.as_deref(),
        }
    }

    pub fn into_root(self) -> std::result::Result<PathBuf, String> {
        match self {
            Self::Result { root, .. } => Ok(PathBuf::from(root)),
            Self::Error { message, .. } => Err(message),
        }
    }
}

#[derive(Clone)]
pub struct Service {
    store: Store,
}

impl Service {
    pub async fn open(store: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: Store::open(store.as_ref().to_path_buf()).await?,
        })
    }

    pub async fn open_default() -> Result<Self> {
        Self::open(default_store_path()?).await
    }

    pub async fn dispatch(
        &self,
        request: Request,
        progress: Option<ProgressSender>,
    ) -> TerminalEvent {
        match request {
            Request::InstallShared(request) => {
                let result = match progress {
                    Some(progress) => {
                        self.store
                            .install_with_progress(&request.home, request.plan, progress)
                            .await
                    }
                    None => self.store.install(&request.home, request.plan).await,
                };
                match result {
                    Ok(root) => TerminalEvent::Result {
                        id: request.id,
                        root: root.to_string_lossy().into_owned(),
                    },
                    Err(error) => TerminalEvent::Error {
                        id: Some(request.id),
                        message: format!("{error:#}"),
                    },
                }
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
        Some(_) => anyhow::bail!("TELORA_EES_STORE must not be empty"),
        None => default_store_path(),
    }
}

pub fn dispatch_blocking(request: Request) -> Result<TerminalEvent> {
    let store = configured_store_path()?;
    thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot start the embedded EES runtime")?
            .block_on(async {
                let service = Service::open(store).await?;
                Result::<_>::Ok(service.dispatch(request, None).await)
            })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("embedded EES runtime panicked"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
