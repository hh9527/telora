use anyhow::{Result, bail, ensure};
use clap::Parser;
use std::{
    io::{BufRead, Read, Write},
    path::PathBuf,
};
use telora_run::{Options, Runner, SourceInput};
const INPUT_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Parser)]
#[command(
    version,
    about = "Execute a telora build artifact without the compiler"
)]
pub struct Cli {
    pub artifact: PathBuf,
    /// Process JSONL requests; every request starts from the initialized state.
    #[arg(long)]
    serve: bool,
    /// NAME=PATH or NAME=file+json://PATH (also yaml/toml); not allowed for snapshots.
    #[arg(long = "source")]
    sources: Vec<String>,
    /// Per-request fuel in millions, overriding the artifact default.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=u64::MAX/1_000_000))]
    with_fuel: Option<u64>,
    /// Per-request memory allowance in MiB, in addition to the initialized memory.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=(usize::MAX as u64)/(1<<20)))]
    with_memory_limit: Option<u64>,
    #[arg(long)]
    report_usage: bool,
    /// Emit separate file/load/initialization/request timings as JSON diagnostics.
    #[arg(long)]
    report_timings: bool,
}

fn read_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    reader
        .take(INPUT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= INPUT_LIMIT,
        "input exceeds {INPUT_LIMIT} bytes"
    );
    Ok(bytes)
}

fn source(value: &str) -> Result<SourceInput> {
    let (name, spec) = value
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("source requires NAME=PATH"))?;
    ensure!(!name.is_empty(), "empty source name");
    let (format, path) = if let Some((scheme, path)) = spec.split_once("://") {
        (
            scheme
                .strip_prefix("file+")
                .ok_or_else(|| anyhow::anyhow!("only file sources are supported"))?,
            path,
        )
    } else {
        (
            std::path::Path::new(spec)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
            spec,
        )
    };
    let format = match format.to_ascii_lowercase().as_str() {
        "json" => 1,
        "yaml" | "yml" => 2,
        "toml" => 3,
        _ => bail!("unknown data source format"),
    };
    Ok(SourceInput {
        name: name.into(),
        data: read_limited(std::fs::File::open(path)?)?,
        format,
    })
}

pub fn diagnostic(value: &serde_json::Value) -> Result<()> {
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    serde_json::to_writer(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}

fn usage(runner: &Runner) -> Result<()> {
    let u = runner.usage();
    diagnostic(
        &serde_json::json!({"schema":"telora.execution/v1","record":"diagnostic",
        "severity":"info","code":"execution-usage","message":"Wasm execution resource usage",
        "labels":[],"notes":[],"usage":{"fuel":{"limit":u.fuel_limit,"consumed":u.fuel_consumed,
            "remaining":u.fuel_limit.saturating_sub(u.fuel_consumed)},
            "linear_memory":{"bytes":u.memory_bytes,"limit_bytes":u.memory_limit_bytes}}}),
    )
}

#[derive(serde::Deserialize)]
struct Reply<'a> {
    schema: String,
    #[serde(borrow)]
    ok: &'a serde_json::value::RawValue,
    error: bool,
    diagnostics: Vec<serde_json::Value>,
}

fn failure(message: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"schema":"telora.service/v1","ok":null,"error":true,
        "diagnostics":[{"severity":"Error","message":message,"labels":[],"notes":[]}]}),
    )
    .expect("JSON value is serializable")
}

fn reply(runner: &mut Runner, input: &[u8]) -> Vec<u8> {
    let response = runner.request(input).and_then(|bytes| {
        let value: Reply<'_> = serde_json::from_slice(&bytes)?;
        ensure!(
            value.schema == "telora.service/v1",
            "invalid service response"
        );
        Ok(bytes)
    });
    match response {
        Ok(value) => value,
        Err(error) => failure(&error.to_string()),
    }
}

pub fn execute(cli: Cli) -> Result<i32> {
    let start = std::time::Instant::now();
    let bytes = std::fs::read(&cli.artifact)?;
    let read_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut runner = Runner::load(
        &bytes,
        Options {
            fuel: cli.with_fuel.map(|n| n * 1_000_000),
            memory_limit: cli.with_memory_limit.map(|n| (n as usize) * (1 << 20)),
        },
    )?;
    drop(bytes);
    ensure!(
        !runner.publication.initialized || cli.sources.is_empty(),
        "snapshot sources are fixed; rebuild to replace them"
    );
    let mut sources = cli
        .sources
        .iter()
        .map(|s| source(s))
        .collect::<Result<Vec<_>>>()?;
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    ensure!(
        !sources.windows(2).any(|p| p[0].name == p[1].name),
        "duplicate source name"
    );
    for d in runner.initialize(&sources)? {
        diagnostic(&d)?;
    }
    drop(sources);
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    if !cli.serve {
        let input = read_limited(stdin)?;
        let response = reply(&mut runner, &input);
        let response: Reply<'_> = serde_json::from_slice(&response)?;
        for d in &response.diagnostics {
            diagnostic(d)?;
        }
        if cli.report_usage {
            usage(&runner)?;
        }
        if cli.report_timings {
            timings(&runner, read_ms)?;
        }
        if response.error {
            return Ok(1);
        }
        stdout.write_all(response.ok.get().as_bytes())?;
        writeln!(stdout)?;
        return Ok(0);
    }
    loop {
        let mut input = vec![];
        let mut oversized = false;
        loop {
            let available = stdin.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let end = available.iter().position(|&b| b == b'\n');
            let count = end.map_or(available.len(), |n| n + 1);
            if input.len().saturating_add(count) <= INPUT_LIMIT {
                input.extend_from_slice(&available[..count]);
            } else {
                oversized = true;
            }
            stdin.consume(count);
            if end.is_some() {
                break;
            }
        }
        if input.is_empty() && !oversized {
            break;
        }
        let response = if oversized {
            failure("request exceeds input size limit")
        } else {
            reply(&mut runner, &input)
        };
        stdout.write_all(&response)?;
        writeln!(stdout)?;
        stdout.flush()?;
        if cli.report_usage {
            usage(&runner)?;
        }
        if cli.report_timings {
            timings(&runner, read_ms)?;
        }
    }
    Ok(0)
}

fn timings(runner: &Runner, read_ms: f64) -> Result<()> {
    diagnostic(
        &serde_json::json!({"schema":"telora.execution/v1","record":"diagnostic",
        "severity":"info","code":"execution-timings","message":"Standalone Wasm phase timings",
        "labels":[],"notes":[],"read_ms":read_ms,"timings":runner.timings}),
    )
}
