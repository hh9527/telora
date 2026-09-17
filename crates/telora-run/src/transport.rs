//! Shared serving transports; execution and reset remain the caller's responsibility.
mod http;
use anyhow::{Result, bail};
use std::{
    io::{BufRead, Write},
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
};

#[derive(Clone, Debug)]
pub enum Bind {
    Stdio,
    Http(SocketAddr),
    Unix(PathBuf),
}

impl FromStr for Bind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        if value == "stdio+jsonl://" {
            return Ok(Self::Stdio);
        }
        if let Some(addr) = value.strip_prefix("http://") {
            return addr
                .parse()
                .map(Self::Http)
                .map_err(|_| "expected http://IP:PORT".into());
        }
        if let Some(path) = value.strip_prefix("http+unix://") {
            if !cfg!(unix) {
                return Err("Unix sockets are unavailable on this platform".into());
            }
            if !path.starts_with('/') || path.contains(['?', '#']) {
                return Err("expected http+unix:///absolute/path.sock".into());
            }
            return Ok(Self::Unix(path.into()));
        }
        Err("expected stdio+jsonl://, http://IP:PORT or http+unix:///absolute/path.sock".into())
    }
}

pub fn failure(message: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"schema":"telora.service/v1","ok":null,"error":true,
        "diagnostics":[{"severity":"Error","message":message,"labels":[],"notes":[]}]}),
    )
    .unwrap()
}

pub fn serve(
    bind: Bind,
    limit: usize,
    mut transform: impl FnMut(&[u8]) -> Result<Vec<u8>>,
) -> Result<()> {
    match bind {
        Bind::Stdio => {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            loop {
                let mut bytes = Vec::new();
                let mut oversized = false;
                loop {
                    let available = input.fill_buf()?;
                    if available.is_empty() {
                        break;
                    }
                    let end = available.iter().position(|&b| b == b'\n');
                    let count = end.map_or(available.len(), |n| n + 1);
                    if bytes.len().saturating_add(count) <= limit {
                        bytes.extend_from_slice(&available[..count]);
                    } else {
                        oversized = true;
                    }
                    input.consume(count);
                    if end.is_some() {
                        break;
                    }
                }
                if bytes.is_empty() && !oversized {
                    return Ok(());
                }
                let response = if oversized {
                    failure("request exceeds input size limit")
                } else {
                    transform(&bytes)?
                };
                output.write_all(&response)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        bind => {
            if limit == 0 {
                bail!("input limit must be positive");
            }
            http::serve(bind, limit, transform)
        }
    }
}
