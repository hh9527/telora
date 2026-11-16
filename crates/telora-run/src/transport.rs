//! Shared serving transports; execution and reset remain the caller's responsibility.
mod http;
use anyhow::{Result, bail};
use std::{
    io::{BufRead, Read, Write},
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
};

#[derive(Clone, Debug)]
pub enum Bind {
    Stdio,
    LocatedStdio,
    Http(SocketAddr),
    Unix(PathBuf),
}

impl FromStr for Bind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        if value == "stdio+located://" {
            return Ok(Self::LocatedStdio);
        }
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
        Err("expected stdio+jsonl://, stdio+located://, http://IP:PORT or http+unix:///absolute/path.sock".into())
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
        Bind::LocatedStdio => serve_frames(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            limit,
            transform,
        ),
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

/// Each request is a u32 little-endian length followed by TLI1 bytes; replies
/// use the same framing around the ordinary UTF-8 service response JSON.
fn serve_frames(
    mut input: impl Read,
    mut output: impl Write,
    limit: usize,
    mut transform: impl FnMut(&[u8]) -> Result<Vec<u8>>,
) -> Result<()> {
    loop {
        let mut size = [0u8; 4];
        if input.read(&mut size[..1])? == 0 {
            return Ok(());
        }
        input.read_exact(&mut size[1..])?;
        let length = u32::from_le_bytes(size) as usize;
        anyhow::ensure!(length <= limit, "request exceeds input size limit");
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        let reply = transform(&bytes)?;
        let size = u32::try_from(reply.len())?;
        output.write_all(&size.to_le_bytes())?;
        output.write_all(&reply)?;
        output.flush()?;
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    #[test]
    fn frames_preserve_binary_newlines_and_request_boundaries() {
        let mut input = Vec::new();
        for bytes in [b"TLI1\n\0".as_slice(), b"TLI2".as_slice()] {
            input.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            input.extend_from_slice(bytes);
        }
        let mut output = Vec::new();
        serve_frames(input.as_slice(), &mut output, 64, |b| Ok(b.to_vec())).unwrap();
        assert_eq!(input, output);
        assert!(serve_frames(&input[..2], Vec::new(), 64, |b| Ok(b.to_vec())).is_err());
        assert!(serve_frames(&input[..7], Vec::new(), 64, |b| Ok(b.to_vec())).is_err());
        assert!(serve_frames(input.as_slice(), Vec::new(), 1, |b| Ok(b.to_vec())).is_err());
    }
}
